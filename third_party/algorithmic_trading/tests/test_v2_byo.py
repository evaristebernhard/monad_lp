"""Bring-your-own strategy, positions, and returns entry points."""

from __future__ import annotations

import json
import textwrap
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

from algotrader.cli import main
from algotrader.data import simulate_ohlcv
from algotrader.lab import LabConfig, run_lab, run_lab_on_positions, run_lab_on_returns
from algotrader.strategies import REGISTRY, get_strategy, load_strategy_plugin, register_strategy, strategy
from algotrader.types import MarketData
from algotrader.verdict import FULL_COMPONENTS, WEIGHTS, reality_score


@pytest.fixture
def synthetic_market() -> MarketData:
    df = simulate_ohlcv("SPY", "2016-01-01", "2023-01-01")
    return MarketData("SIM", df, "synthetic", "1d", "test")


class TestStrategyRegistration:
    def test_register_strategy_and_decorator(self):
        key = "_test_custom_rule"
        REGISTRY.pop(key, None)

        @strategy(key, name="Test Rule", description="decorator path")
        def _rule(df, window: int = 10):
            return (df["close"].pct_change(window) > 0).astype(float)

        assert key in REGISTRY
        assert get_strategy(key).name == "Test Rule"

        REGISTRY.pop(key, None)
        register_strategy(
            key,
            "Registered",
            lambda df: pd.Series(1.0, index=df.index),
            description="imperative path",
        )
        assert get_strategy(key).description == "imperative path"
        REGISTRY.pop(key, None)

    def test_plugin_loader_registers_strategies(self, tmp_path: Path):
        plugin = tmp_path / "my_rule.py"
        plugin.write_text(
            textwrap.dedent(
                """
                from algotrader import register_strategy
                import pandas as pd

                register_strategy(
                    "plugin_rule",
                    "Plugin Rule",
                    lambda df: pd.Series(0.5, index=df.index),
                )
                """
            )
        )
        load_strategy_plugin(plugin)
        assert "plugin_rule" in REGISTRY
        REGISTRY.pop("plugin_rule", None)


class TestCLIByo:
    def test_unknown_strategy_is_rejected_at_runtime(self, capsys):
        code = main([
            "lab", "--strategy", "definitely_not_a_strategy",
            "--source", "synthetic", "--start", "2018-01-01", "--end", "2023-01-01",
            "--permutations", "0", "--quiet",
        ])
        err = capsys.readouterr().err
        assert code == 1
        assert "Unknown strategy" in err

    def test_plugin_strategy_runs(self, tmp_path: Path, capsys):
        plugin = tmp_path / "rule.py"
        plugin.write_text(
            textwrap.dedent(
                """
                from algotrader import register_strategy
                import pandas as pd

                register_strategy("cli_plugin", "CLI Plugin", lambda df: pd.Series(1.0, index=df.index))
                """
            )
        )
        code = main([
            "lab", "--plugin", str(plugin), "--strategy", "cli_plugin",
            "--source", "synthetic", "--start", "2018-01-01", "--end", "2023-01-01",
            "--permutations", "5", "--folds", "2", "--quiet", "--json",
        ])
        payload = json.loads(capsys.readouterr().out)
        assert code == 0
        assert payload["strategy"] == "cli_plugin"
        REGISTRY.pop("cli_plugin", None)

    def test_returns_file_mode(self, tmp_path: Path, capsys):
        idx = pd.date_range("2018-01-01", periods=200, freq="B")
        rets = pd.DataFrame({"ret": np.random.default_rng(1).normal(0.0005, 0.01, len(idx))}, index=idx)
        path = tmp_path / "returns.csv"
        rets.to_csv(path)

        code = main([
            "lab", "--returns-file", str(path), "--source", "synthetic",
            "--permutations", "20", "--n-trials", "3", "--quiet", "--json",
        ])
        payload = json.loads(capsys.readouterr().out)
        assert code == 0
        assert payload["verdict"]["panel_coverage"] == 55
        assert "walk_forward" in payload["verdict"]["unavailable_components"]


class TestPositionsMode:
    def test_positions_backtest_matches_run_lab(self, synthetic_market, monkeypatch):
        monkeypatch.setattr(
            "algotrader.lab.load_ohlcv",
            lambda *a, **k: synthetic_market,
        )
        cfg = LabConfig(
            strategy="sma_cross",
            params={"fast": 20, "slow": 100},
            n_permutations=0,
            wf_folds=2,
            grid_limit=8,
        )
        lab_report = run_lab(cfg)
        strat = get_strategy("sma_cross")
        target = strat.generate(synthetic_market.df, cfg.params)

        pos_report = run_lab_on_positions(
            synthetic_market,
            target,
            cfg=LabConfig(
                n_permutations=15,
                seed=0,
                commission_bps=cfg.commission_bps,
                slippage_bps=cfg.slippage_bps,
                lag=cfg.lag,
                allow_short=cfg.allow_short,
                max_leverage=cfg.max_leverage,
                capital=cfg.capital,
            ),
        )

        assert pos_report.input_mode == "positions"
        assert lab_report.backtest.sharpe == pytest.approx(pos_report.backtest.sharpe, rel=1e-9)
        assert lab_report.backtest.metrics["total_return"] == pytest.approx(
            pos_report.backtest.metrics["total_return"], rel=1e-9
        )
        assert pos_report.verdict["panel_coverage"] == 65


class TestReturnsMode:
    def test_synthetic_positive_sharpe_runs_bootstrap(self):
        idx = pd.date_range("2016-01-01", periods=300, freq="B")
        rng = np.random.default_rng(42)
        rets = pd.Series(0.0008 + 0.008 * rng.standard_normal(len(idx)), index=idx)

        report = run_lab_on_returns(
            rets,
            n_trials=2,
            cfg=LabConfig(n_permutations=40, seed=0),
        )

        assert report.input_mode == "returns"
        assert report.backtest.sharpe > 0.5
        assert report.permutation is not None
        assert report.permutation.method == "stationary_bootstrap"
        assert report.verdict["panel_coverage"] == 55
        assert set(report.verdict["unavailable_components"]) >= {
            "walk_forward", "overfitting", "robustness",
        }


class TestVerdictRenormalisation:
    def test_partial_panel_reweights_components(self):
        metrics = {"n_trades": 120, "sharpe": 1.0, "total_return": 0.4, "max_drawdown": -0.15}
        benchmark = {"sharpe": 0.3}
        out = reality_score(
            metrics,
            benchmark,
            p_value=0.02,
            dsr=0.75,
            available_components={"significance", "selection"},
        )
        sig = out["components"]["significance"]
        sel = out["components"]["selection"]
        expected = (sig * WEIGHTS["significance"] + sel * WEIGHTS["selection"]) / (
            WEIGHTS["significance"] + WEIGHTS["selection"]
        )
        assert out["score"] == pytest.approx(round(expected, 1), abs=0.05)
        assert out["panel_coverage"] == 55
        assert out["unavailable_components"] == sorted(FULL_COMPONENTS - {"significance", "selection"})
        assert "55% of the panel" in out["verdict"]

    def test_full_panel_legacy_score_is_pinned(self):
        """Regression guard: legacy path with all five inputs must not drift."""
        metrics = {
            "n_trades": 250,
            "sharpe": 1.35,
            "total_return": 0.62,
            "max_drawdown": -0.18,
            "turnover_ann": 12.0,
        }
        benchmark = {"sharpe": 0.55}
        out = reality_score(
            metrics,
            benchmark,
            p_value=0.008,
            dsr=0.82,
            pbo=0.12,
            wf_efficiency=0.72,
            wf_win_rate=0.8,
            cost_stress_ratio=0.86,
            benchmark_correlation=0.4,
        )
        assert out == {
            "score": 90.8,
            "grade": "A",
            "headline": "Survives everything we threw at it",
            "verdict": (
                "Grade A. Survives everything we threw at it. The edge is still there after the "
                "permutation test, after charging for every variant tried, and after walking it forward."
            ),
            "components": {
                "significance": 100.0,
                "selection": 82.0,
                "walk_forward": 88.4,
                "overfitting": 84.4,
                "robustness": 100.0,
            },
            "flags": [],
        }

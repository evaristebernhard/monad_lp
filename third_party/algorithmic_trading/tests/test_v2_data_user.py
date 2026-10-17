"""Practitioner-supplied OHLCV loading."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pandas as pd
import pytest

from algotrader.data import load_user_ohlcv
from algotrader.lab import LabConfig, run_lab
from algotrader.panel import load_user_panel
from algotrader.types import OHLCV_COLUMNS


def _bars(n: int = 150, start: str = "2018-01-01") -> pd.DataFrame:
    idx = pd.date_range(start, periods=n, freq="B")
    close = np.linspace(100.0, 130.0, n)
    return pd.DataFrame(
        {
            "open": close * 0.999,
            "high": close * 1.01,
            "low": close * 0.99,
            "close": close,
            "volume": 1_000_000.0,
        },
        index=idx,
    )


class TestLoadUserOhlcv:
    def test_csv_round_trip(self, tmp_path: Path):
        path = tmp_path / "SPY.csv"
        frame = _bars()
        frame.reset_index().rename(columns={"index": "Date"}).to_csv(path, index=False)
        market = load_user_ohlcv(path, symbol="SPY")
        assert market.source == "user"
        assert market.symbol == "SPY"
        assert len(market.df) == len(frame)
        assert list(market.df.columns) == list(OHLCV_COLUMNS)

    def test_parquet_round_trip(self, tmp_path: Path):
        pytest.importorskip("pyarrow")
        path = tmp_path / "SPY.parquet"
        _bars().to_parquet(path)
        market = load_user_ohlcv(path)
        assert market.source == "user"
        assert len(market.df) >= 120

    def test_dataframe_passthrough(self):
        market = load_user_ohlcv(_bars(), symbol="TEST")
        assert market.source == "user"
        assert market.is_real
        assert len(market.df) >= 120

    def test_alias_mapping(self, tmp_path: Path):
        frame = _bars().reset_index()
        frame = frame.rename(
            columns={
                "index": "timestamp",
                "open": "Open",
                "high": "High",
                "low": "Low",
                "close": "Adj Close",
                "volume": "v",
            }
        )
        path = tmp_path / "aliases.csv"
        frame.to_csv(path, index=False)
        market = load_user_ohlcv(path)
        assert len(market.df) >= 120

    def test_close_only_input(self):
        idx = pd.date_range("2018-01-01", periods=150, freq="B")
        frame = pd.DataFrame({"close": np.linspace(50, 75, 150)}, index=idx)
        market = load_user_ohlcv(frame)
        assert (market.df["open"] == market.df["close"]).all()
        assert "intrabar tests are degraded" in market.note
        assert "volume missing" in market.note

    def test_too_few_rows(self):
        with pytest.raises(ValueError, match="at least 120"):
            load_user_ohlcv(_bars(80))

    def test_all_nan_close(self):
        frame = _bars()
        frame["close"] = np.nan
        with pytest.raises(ValueError, match="all missing"):
            load_user_ohlcv(frame)

    def test_non_positive_prices(self):
        frame = _bars()
        frame.loc[frame.index[10], "close"] = 0.0
        with pytest.raises(ValueError, match="Non-positive"):
            load_user_ohlcv(frame)

    def test_duplicate_timestamps(self):
        frame = _bars()
        dup = frame.iloc[[0, 0, 1]].copy()
        dup.index = [frame.index[0], frame.index[0], frame.index[1]]
        with pytest.raises(ValueError, match="Duplicate timestamps"):
            load_user_ohlcv(dup)

    def test_unparseable_dates(self):
        frame = _bars().reset_index()
        frame["Date"] = "not-a-date"
        with pytest.raises(ValueError, match="Could not parse|could not be parsed"):
            load_user_ohlcv(frame.set_index("Date"))

    def test_missing_close_column(self):
        frame = _bars().drop(columns=["close"])
        with pytest.raises(ValueError, match="No close price"):
            load_user_ohlcv(frame)

    def test_missing_file(self, tmp_path: Path):
        with pytest.raises(ValueError, match="File not found"):
            load_user_ohlcv(tmp_path / "missing.csv")

    def test_unsupported_suffix(self, tmp_path: Path):
        path = tmp_path / "data.xlsx"
        path.write_text("x")
        with pytest.raises(ValueError, match="Unsupported file type"):
            load_user_ohlcv(path)

    def test_directory_rejected(self, tmp_path: Path):
        with pytest.raises(ValueError, match="directory"):
            load_user_ohlcv(tmp_path)


class TestUserPanel:
    def test_long_format_file(self, tmp_path: Path):
        rows = []
        for sym in ("AAA", "BBB", "CCC", "DDD"):
            for i, dt in enumerate(pd.date_range("2018-01-01", periods=150, freq="B")):
                rows.append({"date": dt, "symbol": sym, "close": 100 + i * 0.01})
        path = tmp_path / "panel.csv"
        pd.DataFrame(rows).to_csv(path, index=False)
        panel = load_user_panel(path)
        assert len(panel.symbols) == 4
        assert all(panel.sources[s] == "user" for s in panel.symbols)

    def test_directory_of_files(self, tmp_path: Path):
        for sym in ("AAA", "BBB", "CCC", "DDD"):
            _bars().to_csv(tmp_path / f"{sym}.csv")
        panel = load_user_panel(tmp_path)
        assert set(panel.symbols) == {"AAA", "BBB", "CCC", "DDD"}


class TestLabWithUserData:
    def test_end_to_end_user_csv(self, tmp_path: Path, monkeypatch):
        path = tmp_path / "custom.csv"
        _bars().to_csv(path)
        monkeypatch.setenv("ALGOTRADER_OFFLINE", "1")
        report = run_lab(
            LabConfig(
                data=str(path),
                symbol="CUSTOM",
                strategy="sma_cross",
                n_permutations=20,
                wf_folds=2,
                grid_limit=4,
            )
        )
        assert report.market.source == "user"
        assert report.verdict["grade"] in {"A", "B", "C", "D", "F"}

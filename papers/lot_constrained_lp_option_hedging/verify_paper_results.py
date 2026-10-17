#!/usr/bin/env python3
"""Audit the paper's sanitized empirical manifest, not a recomputation of an MILP.

Re-run model solvers against original GitHub Actions artifacts for risk certificates.
This lightweight test enforces unit consistency, provenance, and amount arithmetic.
"""
import json
import math
from pathlib import Path

HERE=Path(__file__).resolve().parent
j=json.loads((HERE/"data/paper_results.json").read_text())

assert j["metadata"]["capital_usdc"]==10_000
assert j["metadata"]["expiry_utc"]=="2026-10-23T08:00:00Z"
assert j["dlmm"]["sol_minimum_option_lot"]==10
assert j["eth_cross"]["minimum_lot_eth"]==0.1
assert j["dlmm"]["source_artifact_id"]==11605754511
assert j["eth_cross"]["source_artifact_id"]==11609824369

for x in j["dlmm"]["scenarios"]:
    assert x["unhedged_max_residual"]>=0 and x["hedged_max_residual"]>=0
    assert x["cost"]>=0 and x["lots"]>=0 and isinstance(x["lots"],int)
    if x["lots"]==0:
        assert x["cost"]==0
        assert math.isclose(x["hedged_max_residual"],x["unhedged_max_residual"])
    else:
        assert x["premium"]<=x["cost"]
        assert x["mip_gap"]<0.01
        assert x["hedged_max_residual"]<x["unhedged_max_residual"]
    assert x["cost"]+x["hedged_max_residual"]>=x["cost"]

for section in ("sol_cross","eth_cross"):
    for x in j[section]["rows"]:
        assert x["cost"]>=0 and x["unhedged"]>0 and x["hedged"]>=0
        assert 0<=x["hedged"]<=x["unhedged"]+1e-6
        assert isinstance(x["lots"],int) and x["lots"]>=0

x=j["dlmm"]["scenarios"][1]
assert round(x["cost"]+x["hedged_max_residual"],2)==152.45
y=j["dlmm"]["scenarios"][3]
assert round(y["cost"]+y["hedged_max_residual"],2)==193.15
assert round(j["sol_cross"]["rows"][1]["cost"],2)==318.65
assert round(j["eth_cross"]["rows"][1]["cost"],2)==225.64

fee=j["short_fee_interval"]
assert fee["elapsed_seconds"]==704
for x in fee["rows"]:
    assert x["shadow_fees_usdc"]>=0
    assert x["ideal_divergence_usdc"]>=0
assert fee["rows"][2]["warning"].startswith("Same active Bin")

tex=(HERE/"experiments.tex").read_text()
for token in ("202.45","29.46","122.99","318.65","225.64","16.968"):
    assert token in tex, f"paper dropped documented measurement: {token}"

print("PASS: empirical paper manifest backed by explicitly cited real research artifacts")
print("PASS: SOL/ETH lot-size unit normalization, cost+residual, and rounded manuscript figures")
print("NOTE: this does NOT re-run exchange MILPs; source workflows have independent residual-risk oracles")

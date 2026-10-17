#!/usr/bin/env python3
"""Compare two independent GitHub Actions Meteora on-chain fee-growth artifacts.

Both source files are public sanitized chain accounts, not wallet Positions.
Gross fee-counter deltas are not a USD LP realized APR. Safe for offline replay.
"""
import argparse,importlib.util,json
from pathlib import Path
THIS=Path(__file__).with_name("dlmm_fee_growth_rpc_probe.py")
spec=importlib.util.spec_from_file_location("real_dlmm_rpc",THIS)
prober=importlib.util.module_from_spec(spec);spec.loader.exec_module(prober)
def audit(before,after):
    output={"before":before.get("collected_utc"),"after":after.get("collected_utc"),
            "status":"ok","per_pool":{},"source":"Two saved successful GitHub Actions public Bin snapshots",
            "limitations":["Counts of fee-growth counters changing are NOT amount of fees",
             "Bin liquidity supplies can change: fixed-share fee extrapolation invalid",
             "Snapshots encompass 5 BinArrays near the active price, not full historical pool",
             "User actual PositionV2 shares and historical activity unavailable."]}
    for name,b in before.get("pools",{}).items():
        a=after.get("pools",{}).get(name)
        if not a:continue
        c=prober.compare_snapshots(b,a)
        output["per_pool"][name]={
            "status":c["status"],
            "previous_slot":c.get("previous_slot"),"current_slot":c.get("current_slot"),
            "previous_active_bin":b.get("active_bin_id"),
            "current_active_bin":a.get("active_bin_id"),
            "previous_total_fee_rate_bps_state":b.get("base_fee_bps",0)+b.get("variable_fee_bps_snapshot",0),
            "current_total_fee_rate_bps_state":a.get("base_fee_bps",0)+a.get("variable_fee_bps_snapshot",0),
            "common_bins":c.get("common_bins"),
            "changed_fee_growth_bins":c.get("changed_fee_growth_bin_count"),
            "bins_changed_liquidity_supply":c.get("bins_changed_liquidity_supply_count"),
            "counter_decreases_or_resets":c.get("counter_decreases_or_resets"),
            "changed_fee_growth_samples":c.get("changed_bins",[])[:12]}
    return output
def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--before",type=Path,required=True)
    ap.add_argument("--after",type=Path,required=True)
    ap.add_argument("--out",type=Path,default=Path("research-artifacts/dlmm_fee_interval_comparison.json"))
    a=ap.parse_args()
    before=json.loads(a.before.read_text());after=json.loads(a.after.read_text())
    result=audit(before,after)
    a.out.parent.mkdir(parents=True,exist_ok=True)
    a.out.write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps({"status":result["status"],"before":result["before"],"after":result["after"],
                     "pools":{k:{z:v for z,v in d.items() if z in (
                      "common_bins","changed_fee_growth_bins","bins_changed_liquidity_supply",
                      "counter_decreases_or_resets","previous_active_bin","current_active_bin")}
                      for k,d in result["per_pool"].items()}},indent=2))
if __name__=="__main__":main()

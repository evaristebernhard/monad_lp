#!/usr/bin/env python3
"""Read-only Meteora DLMM per-Bin fee-growth snapshots from Solana RPC.

SOURCES: current repo rpc/data/rpc.db for configured Solana endpoints (never
printed or uploaded), repo existing DLMM snapshot DB for archival coverage;
live getAccountInfo/getMultipleAccounts via Solana public/program RPC.
Bin fields: Meteora BinArray header 56 bytes, Bin 144 bytes,
fee_amount_x_per_token_stored offset+80 u128, Y offset+96 u128.
Only public pool accounts; NO wallets, user positions, trades, RPC URL leaks.
"""
import argparse, base64, datetime as dt, importlib.util, io, ipaddress, json
import os, re, sqlite3, struct, time, urllib.error, urllib.request, zipfile
from pathlib import Path
from urllib.parse import urlsplit

ROOT=Path(__file__).resolve().parents[1]
PROBE_PATH=ROOT/"rpc/tools/meteora-dlmm-live-probe.py"
sp=importlib.util.spec_from_file_location("dlmm_probe",PROBE_PATH)
PROBE=importlib.util.module_from_spec(sp);sp.loader.exec_module(PROBE)
POOLS={
 "meteora_1bp_SOL_USDC":"HTvjzsfX3yU6BUodCjZ5vZkUrAxMDTrBs3CJaq43ashR",
 "meteora_4bp_SOL_USDC":"5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6",
 "meteora_20bp_SOL_USDC":"BVRbyLjjfSBcoyiYFuxbgKYnWuiFaF9CSXEa5vdSZ9Hh",
}
SOL="So11111111111111111111111111111111111111112"
USDC="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
ALLOWED_SUFFIXES=("quiknode.pro","alchemy.com","helius-rpc.com",
 "rpcpool.com","publicnode.com","solana.com","ankr.com","chainstack.com")
PUBLIC_RPC=("https://api.mainnet-beta.solana.com",
 "https://solana-rpc.publicnode.com")
BINARRAY_SIZE=10136
BIN_SIZE=144
BIN_HEADER=56
def now():return dt.datetime.now(dt.timezone.utc).isoformat()
def db_readonly(path):
    p=Path(path)
    if not p.is_file() or p.open("rb").read(16)!=b"SQLite format 3\x00":
        return None
    out=sqlite3.connect("file:"+str(p.resolve())+"?mode=ro",uri=True)
    out.row_factory=sqlite3.Row;out.execute("PRAGMA query_only=ON")
    return out
def audit_existing(path):
    db=db_readonly(path)
    if not db:return {"status":"missing_sqlite_lfs_bytes"}
    known={}
    for tab in ("pools","pool_snapshots","bin_snapshots","deribit_snapshots"):
        has=db.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",(tab,)).fetchone()
        if not has:continue
        count=db.execute('SELECT count(*) FROM "'+tab+'"').fetchone()[0]
        cols=[x[1] for x in db.execute('PRAGMA table_info("'+tab+'")')]
        known[tab]={"rows":count,
          "cumulative_bin_fee_growth_stored":bool({"fee_amount_x_per_token_stored",
           "fee_amount_y_per_token_stored"}<=set(cols))}
    snap={}
    for name,pool in POOLS.items():
        try:
            row=db.execute("SELECT snapshot_id, observed_at_utc, active_id, bin_step_bps, populated_bin_count FROM pool_snapshots WHERE pool_address=? AND status='ok' ORDER BY snapshot_id DESC LIMIT 1",(pool,)).fetchone()
            snap[name]=dict(row) if row else None
        except sqlite3.Error:snap[name]=None
    db.close()
    return {"status":"ok","tables":known,"latest_selected_pools":snap}
def is_allowlisted(url):
    try:
        z=urlsplit(url)
        if z.scheme!="https" or not z.hostname or z.username or z.password or z.fragment:
            return False
        host=z.hostname.lower().rstrip(".")
        if not any(host==suffix or host.endswith("."+suffix) for suffix in ALLOWED_SUFFIXES):
            return False
        try:
            ip=ipaddress.ip_address(host)
            if ip.is_private or ip.is_loopback:return False
        except ValueError:pass
        return True
    except Exception:return False
def endpoints_from_repo_db(path):
    db=db_readonly(path)
    result=[];meta={"loaded":bool(db),"selected_enabled_urls":0,"credential_like_urls_total":0}
    if not db:return result,meta
    try:
        rows=db.execute("""SELECT e.url,e.tier,c.slug FROM rpc_endpoints e
                       JOIN chains c ON e.chain_id=c.id
                       WHERE e.enabled=1 AND c.enabled=1 AND c.protocol='solana'
                       ORDER BY CASE e.tier WHEN 'free' THEN 0 ELSE 1 END,
                           e.priority ASC LIMIT 80""").fetchall()
        for row in rows:
            raw=str(row["url"]);z=urlsplit(raw)
            if z.query or z.password or (len(z.path)>12 and z.path not in ("/",)):
                meta["credential_like_urls_total"]+=1
            if is_allowlisted(raw):
                result.append(raw)
        meta["selected_enabled_urls"]=len(result)
    except sqlite3.Error:meta["read_error"]="schema_unavailable"
    finally:db.close()
    return result,meta
def rpc_request(url,method,params,timeout=13):
    pay=json.dumps({"jsonrpc":"2.0","id":42,"method":method,"params":params}).encode()
    request=urllib.request.Request(url,pay,{"content-type":"application/json",
                                            "user-agent":"arbitage-bin-fee-growth-public-research/1.0"},
                                   method="POST")
    with urllib.request.urlopen(request,timeout=timeout) as r:data=json.load(r)
    if data.get("error"):
        error=data["error"]
        raise RuntimeError("rpc_error_"+str(error.get("code","unknown")))
    return data.get("result")
def choose_rpc(pool,rpc_db):
    urls,meta=endpoints_from_repo_db(rpc_db)
    # prefer registered free endpoints before known public fallbacks, never report URL.
    candidates=list(dict.fromkeys(urls[:12]+list(PUBLIC_RPC)))
    checked=0; failures={}
    for url in candidates:
        checked+=1
        try:
            r=rpc_request(url,"getAccountInfo",[pool,{"encoding":"base64","commitment":"confirmed"}])
            val=(r or {}).get("value")
            if val and val.get("owner")==PROBE.PROGRAM:
                return url,meta,{"checked":checked,"status":"ok",
                                 "rpc_source":"repo_configured_or_public_fallback"}
            failures["not_program_pool"]=failures.get("not_program_pool",0)+1
        except Exception as e:
            typ=type(e).__name__
            failures[typ]=failures.get(typ,0)+1
    return None,meta,{"checked":checked,"status":"unavailable","error_types":failures}
def array_pda(pool,index):
    from solders.pubkey import Pubkey
    pub=Pubkey.from_string(pool)
    program=Pubkey.from_string(PROBE.PROGRAM)
    pda,_=Pubkey.find_program_address(
        [b"bin_array",bytes(pub),index.to_bytes(8,"little",signed=True)],program)
    return str(pda)
def decode_bin_array(data,pool):
    if len(data)!=BINARRAY_SIZE or data[:8]!=PROBE.BIN_ARRAY_DISCRIMINATOR:
        raise ValueError("bin_array_wrong_size_or_discriminator")
    index=struct.unpack_from("<q",data,8)[0]
    if PROBE.b58(data[24:56])!=pool:raise ValueError("wrong_pool_in_bin_array")
    output={}
    for j in range(70):
        off=BIN_HEADER+j*BIN_SIZE
        amount_x=PROBE.u64(data,off)
        amount_y=PROBE.u64(data,off+8)
        liquidity=PROBE.u128(data,off+32)
        fee_x=PROBE.u128(data,off+80)
        fee_y=PROBE.u128(data,off+96)
        if not any((amount_x,amount_y,liquidity,fee_x,fee_y)):continue
        output[str(index*70+j)]=dict(
           liquidity_supply=str(liquidity),
           amount_x_raw=amount_x,amount_y_raw=amount_y,
           fee_growth_x_raw=str(fee_x),fee_growth_y_raw=str(fee_y))
    return index,output
def snapshot_pool(url,name,pool,radius_arrays=2):
    state_r=rpc_request(url,"getAccountInfo",[pool,{"encoding":"base64","commitment":"confirmed"}])
    val=state_r["value"]
    if val["owner"]!=PROBE.PROGRAM:raise ValueError("wrong_program_owner")
    raw=base64.b64decode(val["data"][0])
    st=PROBE.parse_lb_pair(raw)
    if set((st["token_x_mint"],st["token_y_mint"]))!={SOL,USDC}:
        raise ValueError("not_exact_canonical_SOL_USDC")
    active=int(st["active_id"]);index=active//70
    indices=list(range(index-radius_arrays,index+radius_arrays+1))
    pubkeys=[array_pda(pool,k) for k in indices]
    # minContextSlot >= LbPair account read slot, no future-looking assumptions.
    minslot=int(state_r["context"]["slot"])
    r=rpc_request(url,"getMultipleAccounts",[
          pubkeys,{"encoding":"base64","commitment":"confirmed","minContextSlot":minslot}])
    if not r or not isinstance(r.get("value"),list):
        raise ValueError("unexpected_multiple_accounts_response")
    found={}
    for k,v in zip(indices,r["value"]):
        if v is None:continue # array not allocated
        if v["owner"]!=PROBE.PROGRAM:raise ValueError("wrong_binarray_owner")
        try:
            decoded_idx,bins=decode_bin_array(base64.b64decode(v["data"][0]),pool)
        except ValueError as e:raise ValueError("binarray_decoder_"+type(e).__name__)
        if decoded_idx!=k:raise ValueError("wrong_array_index")
        found.update(bins)
    static=st["static_parameters"];step=st["bin_step_bps"]
    base=static["base_factor"]*step*10*10**static["base_fee_power_factor"]/1e9
    var=st["variable_parameters"]
    ctl=static["variable_fee_control"];acc=var["volatility_accumulator"]
    # Current accumulator state is a snapshot, not an actual executed-swap fee.
    variable_raw=(ctl*((acc*step)**2)+10**11-1)//10**11 if ctl else 0
    variable_fee=variable_raw/1e9
    return dict(status="ok",name=name,pool=pool,
        observed_utc=now(),slot=int(r["context"]["slot"]),
        pool_info_slot=minslot,active_bin_id=active,
        bin_step_bps=step,arrays_requested=len(indices),
        arrays_found=sum(1 for v in r["value"] if v is not None),
        bin_count=len(found),
        token_x_mint=st["token_x_mint"],token_y_mint=st["token_y_mint"],
        base_fee_bps=base*10000,variable_fee_bps_snapshot=variable_fee*10000,
        protocol_share_bps=static["protocol_share"],
        static_parameters=static,variable_parameters=var,
        bin_fee_growth_raw=found,
        warning="Fee growth is Q64-like program fixed point; raw counters are not USD revenue or position APR.")
def compare_snapshots(prior,new):
    if not prior or prior.get("status")!="ok" or new.get("status")!="ok":
        return {"status":"baseline_only"}
    if prior.get("pool")!=new.get("pool"):
        return {"status":"different_pool"}
    if int(new.get("slot") or 0)<=int(prior.get("slot") or 0):
        return {"status":"non_increasing_slot","note":"do not pretend to observe forward fee growth"}
    older=prior.get("bin_fee_growth_raw") or {}
    newer=new.get("bin_fee_growth_raw") or {}
    shared=sorted(set(older)&set(newer),key=int)
    if not shared:return {"status":"no_common_bins","note":"price moved beyond sampled fee-growth arrays; no fee comparison possible"}
    changed=[];resets=[];supply_changed=0
    for k in shared:
        a=older[k];b=newer[k]
        if a.get("liquidity_supply")!=b.get("liquidity_supply"):
            supply_changed+=1
        dx=int(b["fee_growth_x_raw"])-int(a["fee_growth_x_raw"])
        dy=int(b["fee_growth_y_raw"])-int(a["fee_growth_y_raw"])
        if dx<0 or dy<0:
            resets.append(k);continue
        if dx or dy:
            changed.append({"bin_id":int(k),"delta_fee_growth_x_raw":str(dx),
                            "delta_fee_growth_y_raw":str(dy),
                            "supply_changed":a.get("liquidity_supply")!=b.get("liquidity_supply")})
    return {"status":"compared","previous_observed_utc":prior.get("observed_utc"),
      "current_observed_utc":new.get("observed_utc"),
      "previous_slot":prior.get("slot"),"current_slot":new.get("slot"),
      "common_bins":len(shared),"changed_fee_growth_bin_count":len(changed),
      "bins_changed_liquidity_supply_count":supply_changed,
      "counter_decreases_or_resets":len(resets),
      "changed_bins":changed[:120],
      "note":"Raw deltas are real counter changes; not executable APR, pool volume or wallet earnings."}
def get_previous_artifact():
    """Read last GitHub Actions artifact as baseline, without printing token or URL."""
    token=os.getenv("GITHUB_TOKEN");repo=os.getenv("GITHUB_REPOSITORY")
    if not token or not repo or not re.fullmatch(r"[-A-Za-z0-9_.]+/[-A-Za-z0-9_.]+",repo):
        return None,{"status":"no_actions_token"}
    headers={"authorization":"Bearer "+token,"accept":"application/vnd.github+json",
             "user-agent":"arbitage-fee-growth-research"}
    try:
        req=urllib.request.Request("https://api.github.com/repos/"+repo+
            "/actions/artifacts?per_page=50",headers=headers)
        with urllib.request.urlopen(req,timeout=12) as r:lst=json.load(r)
        for z in lst.get("artifacts",[]):
            if not z.get("expired") and z.get("name","").startswith("dlmm-fee-growth-"):
                try:
                    req=urllib.request.Request(z["archive_download_url"],headers=headers)
                    with urllib.request.urlopen(req,timeout=15) as r:contents=r.read(4_000_000)
                    with zipfile.ZipFile(io.BytesIO(contents)) as arc:
                        for file in arc.namelist():
                            if file.endswith("dlmm_bin_fee_growth.json"):
                                record=json.loads(arc.read(file))
                                if record.get("schema_version")==1:
                                    return record,{"status":"previous_artifact_found"}
                except Exception:continue
    except Exception as e:
        return None,{"status":"previous_artifact_lookup_failed","error_type":type(e).__name__}
    return None,{"status":"no_previous_artifact"}
def check_layout():
    # Deterministic 10,136-byte mock array; write independently identifiable fields
    data=bytearray(BINARRAY_SIZE)
    data[:8]=PROBE.BIN_ARRAY_DISCRIMINATOR
    # encode fake matching pool via solders public key
    from solders.pubkey import Pubkey
    pool=POOLS["meteora_4bp_SOL_USDC"]
    data[24:56]=bytes(Pubkey.from_string(pool))
    struct.pack_into("<q",data,8,-80)
    off=BIN_HEADER+3*BIN_SIZE
    struct.pack_into("<Q",data,off,27)
    struct.pack_into("<Q",data,off+8,42)
    for ofs,num in ((32,2**75+9),(80,2**91+17),(96,2**90+21)):
        data[off+ofs:off+ofs+16]=num.to_bytes(16,"little")
    idx,bins=decode_bin_array(bytes(data),pool)
    b=bins[str(-80*70+3)]
    assert idx==-80 and b["amount_x_raw"]==27 and b["amount_y_raw"]==42
    assert int(b["liquidity_supply"])==2**75+9
    assert int(b["fee_growth_x_raw"])==2**91+17
    assert int(b["fee_growth_y_raw"])==2**90+21
    x={"status":"ok","pool":pool,"observed_utc":"t0","slot":2,"bin_fee_growth_raw":bins}
    newer=json.loads(json.dumps(x));newer["observed_utc"]="t1";newer["slot"]=3
    newer["bin_fee_growth_raw"][str(-80*70+3)]["fee_growth_x_raw"]=str(2**91+27)
    comparison=compare_snapshots(x,newer)
    assert comparison["changed_fee_growth_bin_count"]==1
    assert compare_snapshots(newer,x)["status"]=="non_increasing_slot"
    assert comparison["changed_bins"][0]["delta_fee_growth_x_raw"]=="10"
    assert is_allowlisted("https://api.mainnet-beta.solana.com")
    assert not is_allowlisted("http://127.0.0.1:8545/rpc/solana-mainnet")
    assert not is_allowlisted("https://evil.test@api.mainnet-beta.solana.com/")
    print("PASS official Bin 144-byte u128 fee offsets, negative-bin indexing, delta audit and RPC safety")
def main():
    a=argparse.ArgumentParser()
    a.add_argument("--self-test",action="store_true")
    a.add_argument("--rpc-db",default="rpc/data/rpc.db")
    a.add_argument("--old-db",default="data/sol_dlmm_deribit.db")
    a.add_argument("--out",default="research-artifacts/dlmm_bin_fee_growth.json")
    a.add_argument("--radius-arrays",type=int,default=2)
    a.add_argument("--prior-local",default="",help="cached sanitized previous sample JSON")
    args=a.parse_args()
    if args.self_test:return check_layout()
    prior=None; pstatus={"status":"no_local_previous"}
    if args.prior_local and Path(args.prior_local).is_file():
        try:
            prior=json.loads(Path(args.prior_local).read_text())
            if prior.get("schema_version")!=1:raise ValueError("prior_schema_mismatch")
            pstatus={"status":"cached_previous_snapshot"}
        except Exception as exc:
            prior=None;pstatus={"status":"cache_invalid","error_type":type(exc).__name__}
    if prior is None:prior,pstatus=get_previous_artifact()
    report={"schema_version":1,"collected_utc":now(),"historic_db_coverage":
      audit_existing(args.old_db),"previous_artifact":pstatus,
      "sampling":"Near-active BinArray PDAs, not entire pool or historical per-bin fees",
      "pools":{},"comparisons":{},
      "risk_notice":"No wallet PositionV2 or real fee payout; future fee budgets A_i not calibrated yet."}
    url,config,status=choose_rpc(POOLS["meteora_4bp_SOL_USDC"],args.rpc_db)
    report["configured_rpc_status"]=config
    report["rpc_probe"]=status
    if url:
        for label,pool in POOLS.items():
            try:
                new=snapshot_pool(url,label,pool,args.radius_arrays)
                report["pools"][label]=new
                old=(prior.get("pools",{}) or {}).get(label) if prior else None
                report["comparisons"][label]=compare_snapshots(old,new)
                print(json.dumps({"pool":label,"status":"ok","slot":new["slot"],
                   "bin_count":new["bin_count"],"base_fee_bps":new["base_fee_bps"],
                   "variable_fee_bps":new["variable_fee_bps_snapshot"],
                   "protocol_share_bps":new["protocol_share_bps"],
                   "fee_growth_delta_status":report["comparisons"][label]["status"],
                   "changed_bins":report["comparisons"][label].get("changed_fee_growth_bin_count")}),flush=True)
            except Exception as e:
                report["pools"][label]={"status":"unavailable","error_type":type(e).__name__,
                         "note":"RPC account unavailable or decoding refused. No data invented."}
                print(json.dumps({"pool":label,"status":"unavailable","type":type(e).__name__}),flush=True)
    else:print(json.dumps({"rpc_status":"unavailable","no_live_fee_growth_fabricated":True}))
    p=Path(args.out);p.parent.mkdir(parents=True,exist_ok=True)
    p.write_text(json.dumps(report,indent=2)+"\n")
    return 0 if any(x.get("status")=="ok" for x in report["pools"].values()) else 2
if __name__=="__main__":raise SystemExit(main() or 0)

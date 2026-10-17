#!/usr/bin/env python3
"""Read-only Meteora DLMM pool/position/bin probe.

The probe never prints or serializes the upstream RPC URL. Position and bin
decoding follows the checked-in Meteora DLMM IDL (PositionV2 and BinArray).
"""
import argparse,base64,json,struct,urllib.request
from datetime import datetime,timezone

PROGRAM="LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo"
ALPHABET="123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
SOL_MINTS={"So11111111111111111111111111111111111111112"}
POSITION_DISCRIMINATOR=bytes([117,176,212,199,245,180,133,182])
BIN_ARRAY_DISCRIMINATOR=bytes([92,142,92,220,5,148,70,181])
POSITION_SIZE=8120
POSITION_BIN_DATA_SIZE=112
BIN_ARRAY_SIZE=10136
BIN_ARRAY_HEADER_SIZE=56
BIN_SIZE=144
Q64=float(1<<64)

def b58(raw):
    n=int.from_bytes(raw,"big");s=""
    while n:
        n,r=divmod(n,58);s=ALPHABET[r]+s
    return "1"*(len(raw)-len(raw.lstrip(b"\x00")))+s

def u64(data,offset):return struct.unpack_from("<Q",data,offset)[0]
def u128(data,offset):return int.from_bytes(data[offset:offset+16],"little")
def i32(data,offset):return struct.unpack_from("<i",data,offset)[0]
def parse_mint(data):
    if len(data)<45:raise ValueError("mint account truncated")
    return {"decimals":data[44]}

def parse_position(data,address=""):
    if len(data)<POSITION_SIZE or (len(data)-POSITION_SIZE)%POSITION_BIN_DATA_SIZE:
        raise ValueError("PositionV2 account has unexpected size")
    if data[:8]!=POSITION_DISCRIMINATOR:raise ValueError("account is not a Meteora DLMM PositionV2")
    share_offset=72; lower_offset=7912
    shares=[u128(data,share_offset+16*i) for i in range(70)]
    lower=i32(data,lower_offset); upper=i32(data,lower_offset+4)
    if upper<lower:raise ValueError("position bin range is inverted")
    width=upper-lower+1
    if width>70+(len(data)-POSITION_SIZE)//POSITION_BIN_DATA_SIZE:
        raise ValueError("position bin range exceeds account capacity")
    for i in range(70,width):
        shares.append(u128(data,POSITION_SIZE+POSITION_BIN_DATA_SIZE*(i-70)))
    return {"position":address,"lb_pair":b58(data[8:40]),"owner":b58(data[40:72]),
            "lower_bin_id":lower,"upper_bin_id":upper,
            "liquidity_shares":[{"bin_id":lower+i,"liquidity_share":str(share)}
                for i,share in enumerate(shares) if share and lower+i<=upper],
            "last_updated_at":struct.unpack_from("<q",data,lower_offset+16)[0],
            "total_claimed_fee_x_raw":u64(data,lower_offset+24),
            "total_claimed_fee_y_raw":u64(data,lower_offset+32)}

def parse_bin_array(data,pool,decimals_x,decimals_y,bin_step):
    if len(data)!=BIN_ARRAY_SIZE:raise ValueError("BinArray account has unexpected size")
    if data[:8]!=BIN_ARRAY_DISCRIMINATOR:raise ValueError("account is not a Meteora DLMM BinArray")
    index=struct.unpack_from("<q",data,8)[0]; lb_pair=b58(data[24:56])
    if lb_pair!=pool:raise ValueError("BinArray belongs to a different pool")
    bins=[]
    for slot in range(70):
        offset=BIN_ARRAY_HEADER_SIZE+slot*BIN_SIZE
        amount_x=u64(data,offset); amount_y=u64(data,offset+8)
        price_raw=u128(data,offset+16)/Q64; liquidity_supply=u128(data,offset+32)
        if not(amount_x or amount_y or liquidity_supply):continue
        # Decimal normalization converts quote raw units/base raw unit to
        # human Y per human X.
        bins.append({"bin_id":index*70+slot,"slot":slot,
            "price_y_per_x_raw":price_raw,
            "price_y_per_x":price_raw*(10**(decimals_x-decimals_y)),
            "amount_x_raw":amount_x,"amount_y_raw":amount_y,
            "liquidity_supply":str(liquidity_supply)})
    return {"bin_array_index":index,"lb_pair":lb_pair,"bins":bins,"bin_step_bps":bin_step}

def parse_lb_pair(data):
    if len(data)<152:raise ValueError("LB pair account truncated")
    active=struct.unpack_from("<i",data,76)[0]
    step=struct.unpack_from("<H",data,80)[0]
    if not(1<=step<=10000):raise ValueError("invalid bin step")
    return {"active_id":active,"bin_step_bps":step,"token_x_mint":b58(data[88:120]),"token_y_mint":b58(data[120:152]),
        "raw_price_y_per_x":(1+step/10000)**active,"raw_account_bytes":len(data),
        "static_parameters":{"base_factor":struct.unpack_from("<H",data,8)[0],
            "filter_period":struct.unpack_from("<H",data,10)[0],"decay_period":struct.unpack_from("<H",data,12)[0],
            "reduction_factor":struct.unpack_from("<H",data,14)[0],"variable_fee_control":struct.unpack_from("<I",data,16)[0],
            "max_volatility_accumulator":struct.unpack_from("<I",data,20)[0],"min_bin_id":i32(data,24),"max_bin_id":i32(data,28),
            "protocol_share":struct.unpack_from("<H",data,32)[0],"base_fee_power_factor":data[34],
            "function_type":data[36],"collect_fee_mode":data[37]},
        "variable_parameters":{"volatility_accumulator":struct.unpack_from("<I",data,40)[0],
            "volatility_reference":struct.unpack_from("<I",data,44)[0],"index_reference":i32(data,48),
            "last_update_timestamp":struct.unpack_from("<q",data,56)[0]}}

def request(url,method,params):
    payload=json.dumps({"jsonrpc":"2.0","id":1,"method":method,"params":params}).encode()
    req=urllib.request.Request(url,data=payload,headers={"content-type":"application/json"},method="POST")
    with urllib.request.urlopen(req,timeout=22) as response:result=json.load(response)
    if result.get("error"):raise RuntimeError(f"RPC {method} returned an error")
    return result.get("result")

def account(url,address):
    value=request(url,"getAccountInfo",[address,{"encoding":"base64","commitment":"confirmed"}]).get("value")
    if not value:raise RuntimeError("pool not available")
    return value.get("owner"),base64.b64decode(value["data"][0])

def rpc(url,pool,position=None,owner=None):
    pool_owner,pool_data=account(url,pool)
    if pool_owner!=PROGRAM:raise RuntimeError("account is not a Meteora DLMM pair")
    result=parse_lb_pair(pool_data)
    _,x_data=account(url,result["token_x_mint"]); _,y_data=account(url,result["token_y_mint"])
    result["token_x_decimals"]=parse_mint(x_data)["decimals"]
    result["token_y_decimals"]=parse_mint(y_data)["decimals"]
    if position:
        position_addresses=[position]
    elif not owner:
        # A pool-only snapshot is the safe default. Scanning every PositionV2
        # account is an expensive indexer-style query; use --owner for a
        # user-scoped Position/Bin snapshot.
        result["positions"]=[]
        result["position_query"]="not_requested"
        return result
    else:
        filters=[{"dataSize":POSITION_SIZE},{"memcmp":{"offset":8,"bytes":pool}}]
        filters.append({"memcmp":{"offset":40,"bytes":owner}})
        found=request(url,"getProgramAccounts",[PROGRAM,{"encoding":"base64","commitment":"confirmed","filters":filters}])
        position_addresses=[item["pubkey"] for item in found]
    positions=[]
    for address in position_addresses:
        position_owner,position_data=account(url,address)
        if position_owner!=PROGRAM:continue
        parsed=parse_position(position_data,address)
        if parsed["lb_pair"]!=pool:raise ValueError("position does not belong to requested pool")
        positions.append(parsed)
    result["positions"]=positions
    if not positions:return result
    filters=[{"dataSize":BIN_ARRAY_SIZE},{"memcmp":{"offset":24,"bytes":pool}}]
    arrays=request(url,"getProgramAccounts",[PROGRAM,{"encoding":"base64","commitment":"confirmed","filters":filters}])
    parsed_arrays=[parse_bin_array(base64.b64decode(item["account"]["data"][0]),pool,
        result["token_x_decimals"],result["token_y_decimals"],result["bin_step_bps"]) for item in arrays]
    by_bin={entry["bin_id"]:entry for array in parsed_arrays for entry in array["bins"]}
    for pos in positions:
        enriched=[]
        for entry in pos["liquidity_shares"]:
            current=by_bin.get(entry["bin_id"])
            if not current:continue
            supply=int(current["liquidity_supply"])
            if not supply:continue
            share=int(entry["liquidity_share"])
            amount_x=current["amount_x_raw"]*share/supply; amount_y=current["amount_y_raw"]*share/supply
            enriched.append({"bin_id":entry["bin_id"],"strike":current["price_y_per_x"],
                "position_share":share/supply,"amount_x_raw":amount_x,"amount_y_raw":amount_y,
                "delta_sol":amount_x/(10**result["token_x_decimals"]) if result["token_x_mint"] in SOL_MINTS else None})
        pos["bin_weights"]=enriched
    result["bin_arrays"]=len(parsed_arrays)
    return result

def main():
    p=argparse.ArgumentParser()
    p.add_argument("--pool",default="5rCf1DM8LjKTw4YqhnoLcngyZYeNnQqztScTogYHAS6")
    p.add_argument("--position",default="",help="optional PositionV2 account")
    p.add_argument("--owner",default="",help="optional position owner; used with getProgramAccounts")
    p.add_argument("--local-rpc",default="http://127.0.0.1:8545/rpc/solana-mainnet")
    p.add_argument("--out",default="")
    a=p.parse_args()
    result={"pool":a.pool,"checked_utc":datetime.now(timezone.utc).isoformat()}
    try:result.update(rpc(a.local_rpc,a.pool,a.position or None,a.owner or None));result["status"]="ok"
    except Exception as e:result["status"]="unavailable";result["error_type"]=type(e).__name__
    # local-rpc is configured to use upstream from rpc.db; do not print or serialize that URL.
    print(json.dumps(result,indent=2))
    if a.out:
        from pathlib import Path
        Path(a.out).write_text(json.dumps(result,indent=2)+"\n")
    return 0 if result["status"]=="ok" else 1
if __name__=="__main__":raise SystemExit(main())

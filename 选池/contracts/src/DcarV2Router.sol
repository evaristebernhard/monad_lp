// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// @notice Integer port of DCAR's candidate-subset allocation core.
/// @dev Research solver for <=6 independent same-pair V2 pools and <=3 splits.
/// Enumerates candidate subsets and uses Q64 marginal water-filling plus a
/// capacity-safe residual allocation. Numeric approximation, not a dual proof.
/// Reserves are caller-supplied observations. No swaps, custody, or oracle claim.
contract DcarV2Router {
    uint256 private constant SCALE = 1 << 64;
    uint256 public constant MAX_VALUE = type(uint80).max;
    struct Pool {
        bytes32 id;
        uint256 reserveIn;
        uint256 reserveOut;
        uint256 feeBps;
        uint256 gasCost;
        uint256 weightBps;
        uint256 capIn;
    }

    function _sqrt(uint256 value) private pure returns (uint256 result) {
        if (value == 0) return 0;
        uint256 x=value;
        result=1;
        if (x>>128>0) {x>>=128;result<<=64;}
        if (x>>64>0) {x>>=64;result<<=32;}
        if (x>>32>0) {x>>=32;result<<=16;}
        if (x>>16>0) {x>>=16;result<<=6;}
        if (x>>8>0) {x>>=8;result<<=4;}
        if (x>>4>0) {x>>=4;result<<=2;}
        if (x>>2>0) result<<=1;
        unchecked {
            for(uint256 i;i<7;i++) result=(result+value/result)>>1;
            uint256 other=value/result;
            if(other<result) result=other;
        }
    }

    function _quote(Pool calldata pool,uint256 x) private pure returns(uint256) {
        uint256 adjusted=(10000-pool.feeBps)*x;
        return pool.reserveOut*adjusted/(10000*pool.reserveIn+adjusted);
    }
    function _marginal(Pool calldata pool,uint256 x) private pure returns(uint256) {
        uint256 fee=10000-pool.feeBps;
        uint256 denominator=10000*pool.reserveIn+fee*x;
        return pool.weightBps*pool.reserveOut*fee*pool.reserveIn*SCALE/(denominator*denominator);
    }
    function _inverse(Pool calldata pool,uint256 mu,uint256 amount) private pure returns(uint256) {
        uint256 fee=10000-pool.feeBps;
        uint256 root=_sqrt(pool.weightBps*pool.reserveOut*fee*pool.reserveIn*SCALE/mu);
        uint256 start=10000*pool.reserveIn;
        if(root<=start) return 0;
        uint256 value=(root-start)/fee;
        uint256 cap=pool.capIn<amount?pool.capIn:amount;
        return value<cap?value:cap;
    }

    function _split(Pool[] calldata pools,uint256 mask,uint256 amount)
        private pure returns(uint256[] memory xs,bool feasible)
    {
        uint256 n=pools.length;
        xs=new uint256[](n);
        uint256 capacity;uint256 high;
        for(uint256 i;i<n;i++) if(mask&(1<<i)!=0) {
            capacity+=pools[i].capIn<amount?pools[i].capIn:amount;
            uint256 marginal=_marginal(pools[i],0);
            if(marginal>high)high=marginal;
        }
        if(capacity<amount)return(xs,false);
        // +1 gives a valid upper bracket with no positive allocation.
        require(high>0,"marginal precision");
        high+=1;
        uint256 low=1;
        for(uint256 round;round<160 && low<high;round++) {
            uint256 mid=low+(high-low)/2;
            uint256 total;
            for(uint256 i;i<n;i++)if(mask&(1<<i)!=0)total+=_inverse(pools[i],mid,amount);
            if(total>amount)low=mid+1;else high=mid;
        }
        uint256 assigned;
        for(uint256 i;i<n;i++)if(mask&(1<<i)!=0){xs[i]=_inverse(pools[i],high,amount);assigned+=xs[i];}
        require(assigned<=amount,"allocation bracket");
        uint256 remaining=amount-assigned;
        for(uint256 pass;remaining>0 && pass<n;pass++) {
            uint256 selected=type(uint256).max;uint256 best;
            for(uint256 i;i<n;i++)if(mask&(1<<i)!=0 && xs[i]<pools[i].capIn){
                uint256 marginal=_marginal(pools[i],xs[i]);
                if(selected==type(uint256).max||marginal>best){selected=i;best=marginal;}
            }
            require(selected!=type(uint256).max,"capacity");
            uint256 room=pools[selected].capIn-xs[selected];
            uint256 extra=remaining<room?remaining:room;
            xs[selected]+=extra;remaining-=extra;
        }
        require(remaining==0,"input conservation");
        return(xs,true);
    }

    function solve(Pool[] calldata pools,uint256 amount,uint8 maxSplits,uint256 baseGas)
        external pure returns(uint256[] memory allocations,uint256 expectedOutput,int256 score,bytes32 inputHash)
    {
        uint256 n=pools.length;
        require(n>0&&n<=6,"pool count");
        require(amount>0&&amount<=MAX_VALUE&&baseGas<=MAX_VALUE,"amount bound");
        require(maxSplits>0&&maxSplits<=3,"splits");
        for(uint256 i;i<n;i++){
            Pool calldata p=pools[i];
            require(p.id!=bytes32(0),"pool id");
            require(p.reserveIn>0&&p.reserveIn<=MAX_VALUE&&p.reserveOut>0&&p.reserveOut<=MAX_VALUE,"reserve bound");
            require(p.feeBps<10000&&p.weightBps>0&&p.weightBps<=10000,"fee or weight");
            require(p.capIn>0&&p.capIn<=MAX_VALUE&&p.gasCost<=MAX_VALUE,"cap or gas");
            for(uint256 j;j<i;j++)require(p.id!=pools[j].id,"duplicate pool");
        }
        score=type(int256).min;
        for(uint256 mask=1;mask<(1<<n);mask++){
            uint256 bits;uint256 value=mask;
            while(value!=0){bits++;value&=value-1;}
            if(bits>maxSplits)continue;
            (uint256[] memory xs,bool feasible)=_split(pools,mask,amount);
            if(!feasible)continue;
            uint256 output;uint256 weighted;uint256 gasCost=baseGas;
            for(uint256 i;i<n;i++)if(xs[i]>0){
                uint256 out=_quote(pools[i],xs[i]);
                output+=out;weighted+=out*pools[i].weightBps/10000;gasCost+=pools[i].gasCost;
            }
            int256 candidate=int256(weighted)-int256(gasCost);
            if(candidate>score){score=candidate;allocations=xs;expectedOutput=output;}
        }
        require(allocations.length>0,"infeasible");
        inputHash=keccak256(abi.encode(pools,amount,maxSplits,baseGas));
    }
}

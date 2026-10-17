// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/DcarV2Router.sol";
interface DcarVm {function expectRevert(bytes calldata) external;}
contract DcarV2RouterTest {
    DcarVm constant vm=DcarVm(address(uint160(uint256(keccak256("hevm cheat code")))));
    DcarV2Router router;
    function setUp() public {router=new DcarV2Router();}
    function pools() private pure returns(DcarV2Router.Pool[] memory p){
        p=new DcarV2Router.Pool[](3);
        p[0]=DcarV2Router.Pool(bytes32(uint256(1)),1000,1000,30,1,10000,1000);
        p[1]=DcarV2Router.Pool(bytes32(uint256(2)),1000,1100,5,1,10000,1000);
        p[2]=DcarV2Router.Pool(bytes32(uint256(3)),1000,900,100,1,10000,1000);
    }
    function quote(DcarV2Router.Pool memory p,uint256 x) private pure returns(uint256){
        uint256 a=(10000-p.feeBps)*x;return p.reserveOut*a/(10000*p.reserveIn+a);
    }
    function testSinglePoolExactQuote() public view {
        DcarV2Router.Pool[] memory p=pools();
        (uint256[] memory xs,uint256 out,int256 score,)=router.solve(p,100,1,2);
        uint256 sum;uint256 active;uint256 expected;
        for(uint256 i;i<3;i++)if(xs[i]>0){sum+=xs[i];active++;expected+=quote(p[i],xs[i]);}
        require(sum==100&&active==1&&out==expected&&score==int256(out)-3);
    }
    function testSplitConservesAndRespectsCaps() public view {
        DcarV2Router.Pool[] memory p=pools();for(uint256 i;i<3;i++)p[i].capIn=40;
        (uint256[] memory xs,,int256 score,bytes32 hash)=router.solve(p,100,3,0);
        uint256 sum;for(uint256 i;i<3;i++){require(xs[i]<=40);sum+=xs[i];}
        require(sum==100&&hash==keccak256(abi.encode(p,uint256(100),uint8(3),uint256(0)))&&score>0);
    }
    function testAgainstIntegerBruteForce() public view {
        DcarV2Router.Pool[] memory p=pools();
        (, ,int256 score,)=router.solve(p,20,3,0);
        int256 best=type(int256).min;
        for(uint256 a;a<=20;a++)for(uint256 b;b<=20-a;b++){
            uint256 c=20-a-b;int256 s=int256(quote(p[0],a)+quote(p[1],b)+quote(p[2],c));
            if(a>0)s-=1;if(b>0)s-=1;if(c>0)s-=1;
            if(s>best)best=s;
        }
        // Numerical water-filling is not claimed to solve every floor-rounded
        // discrete objective exactly. Test the small rounding envelope explicitly.
        require(score<=best&&score+3>=best);
    }
    function testDuplicateRejected() public {
        DcarV2Router.Pool[] memory p=pools();p[1].id=p[0].id;
        vm.expectRevert(bytes("duplicate pool"));router.solve(p,100,3,0);
    }
    function testInfeasibleRejected() public {
        DcarV2Router.Pool[] memory p=pools();for(uint256 i;i<3;i++)p[i].capIn=1;
        vm.expectRevert(bytes("infeasible"));router.solve(p,100,3,0);
    }
    function testOverflowBoundsRejected() public {
        DcarV2Router.Pool[] memory p=pools();p[0].reserveIn=uint256(type(uint80).max)+1;
        vm.expectRevert(bytes("reserve bound"));router.solve(p,100,3,0);
    }
    function testZeroCapRejected() public {
        DcarV2Router.Pool[] memory p=pools();p[0].capIn=0;
        vm.expectRevert(bytes("cap or gas"));router.solve(p,100,3,0);
    }
    function testGasCanMakeScoreNegative() public view {
        DcarV2Router.Pool[] memory p=pools();
        (, ,int256 score,)=router.solve(p,1,1,1000);
        require(score<0);
    }
    function testSixCandidates() public view {
        DcarV2Router.Pool[] memory p=new DcarV2Router.Pool[](6);
        for(uint256 i;i<6;i++)p[i]=DcarV2Router.Pool(bytes32(i+1),1000000,1000000+i*1000,30,1,10000,1000000);
        (uint256[] memory xs,,,)=router.solve(p,10000,3,0);
        uint256 sum;uint256 active;for(uint256 i;i<6;i++){sum+=xs[i];if(xs[i]>0)active++;}
        require(sum==10000&&active<=3);
    }
}

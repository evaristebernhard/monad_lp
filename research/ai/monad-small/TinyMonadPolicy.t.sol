// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import "../src/TinyMonadPolicy.sol";
contract TinyMonadPolicyTest {
    TinyMonadPolicy model;
    function setUp() public { model = new TinyMonadPolicy(); }
    function testRealVector0() public {
        int32[12] memory input;
        input[0] = 5051;
        input[1] = -10891;
        input[2] = -11697;
        input[3] = -8551;
        input[4] = -7757;
        input[5] = 7702;
        input[6] = 5001;
        input[7] = -9463;
        input[8] = -9922;
        input[9] = 10685;
        input[10] = 6590;
        input[11] = 5671;
        (int256[3] memory scores, uint8 action) = model.predict(input);
        require(scores[0] == -9435, "score-0");
        require(scores[1] == 12488, "score-1");
        require(scores[2] == 8165, "score-2");
        require(action == 1, "action");
    }
    function testRealVector1() public {
        int32[12] memory input;
        input[0] = -4413;
        input[1] = -13903;
        input[2] = -11773;
        input[3] = -6604;
        input[4] = -9506;
        input[5] = 671;
        input[6] = -4343;
        input[7] = -9409;
        input[8] = -11127;
        input[9] = 10685;
        input[10] = 6588;
        input[11] = 671;
        (int256[3] memory scores, uint8 action) = model.predict(input);
        require(scores[0] == -10238, "score-0");
        require(scores[1] == 11508, "score-1");
        require(scores[2] == 6444, "score-2");
        require(action == 1, "action");
    }
    function testRealVector2() public {
        int32[12] memory input;
        input[0] = -251;
        input[1] = 951;
        input[2] = 1551;
        input[3] = -2262;
        input[4] = -421;
        input[5] = -2252;
        input[6] = -292;
        input[7] = 1317;
        input[8] = -642;
        input[9] = -1298;
        input[10] = -1447;
        input[11] = -5488;
        (int256[3] memory scores, uint8 action) = model.predict(input);
        require(scores[0] == 2067, "score-0");
        require(scores[1] == -1682, "score-1");
        require(scores[2] == -1481, "score-2");
        require(action == 0, "action");
    }
    function testRealVector3() public {
        int32[12] memory input;
        input[0] = -1166;
        input[1] = -311;
        input[2] = 1769;
        input[3] = -1485;
        input[4] = -859;
        input[5] = -1747;
        input[6] = -1190;
        input[7] = -211;
        input[8] = -1124;
        input[9] = -1482;
        input[10] = -1429;
        input[11] = -1831;
        (int256[3] memory scores, uint8 action) = model.predict(input);
        require(scores[0] == 1692, "score-0");
        require(scores[1] == -1062, "score-1");
        require(scores[2] == -1350, "score-2");
        require(action == 0, "action");
    }
    function testRealVector4() public {
        int32[12] memory input;
        input[0] = -178;
        input[1] = 63;
        input[2] = 395;
        input[3] = -1426;
        input[4] = -690;
        input[5] = -2238;
        input[6] = -219;
        input[7] = 332;
        input[8] = -742;
        input[9] = -2286;
        input[10] = -2390;
        input[11] = -7327;
        (int256[3] memory scores, uint8 action) = model.predict(input);
        require(scores[0] == 3649, "score-0");
        require(scores[1] == -2641, "score-1");
        require(scores[2] == -2059, "score-2");
        require(action == 0, "action");
    }
    function testRealVector5() public {
        int32[12] memory input;
        input[0] = -69;
        input[1] = 239;
        input[2] = 671;
        input[3] = -1230;
        input[4] = -586;
        input[5] = -2050;
        input[6] = -113;
        input[7] = 56;
        input[8] = -760;
        input[9] = -2437;
        input[10] = -2396;
        input[11] = -2093;
        (int256[3] memory scores, uint8 action) = model.predict(input);
        require(scores[0] == 3308, "score-0");
        require(scores[1] == -1782, "score-1");
        require(scores[2] == -1841, "score-2");
        require(action == 0, "action");
    }
    function testRealVector6() public {
        int32[12] memory input;
        input[0] = 1748;
        input[1] = 1419;
        input[2] = 879;
        input[3] = 481;
        input[4] = -847;
        input[5] = -857;
        input[6] = 1687;
        input[7] = 1300;
        input[8] = 205;
        input[9] = -2110;
        input[10] = -2229;
        input[11] = 2206;
        (int256[3] memory scores, uint8 action) = model.predict(input);
        require(scores[0] == 2266, "score-0");
        require(scores[1] == -569, "score-1");
        require(scores[2] == 78, "score-2");
        require(action == 0, "action");
    }
    function testRealVector7() public {
        int32[12] memory input;
        input[0] = 2684;
        input[1] = 1454;
        input[2] = 2123;
        input[3] = 1595;
        input[4] = -336;
        input[5] = 821;
        input[6] = 2620;
        input[7] = 2220;
        input[8] = 1379;
        input[9] = -1707;
        input[10] = -2085;
        input[11] = 5049;
        (int256[3] memory scores, uint8 action) = model.predict(input);
        require(scores[0] == 423, "score-0");
        require(scores[1] == 1006, "score-1");
        require(scores[2] == 955, "score-2");
        require(action == 1, "action");
    }
    function testRealVector8() public {
        int32[12] memory input;
        input[0] = 6013;
        input[1] = 4669;
        input[2] = 3793;
        input[3] = 1865;
        input[4] = -2474;
        input[5] = 1977;
        input[6] = 5977;
        input[7] = 4629;
        input[8] = 1041;
        input[9] = 554;
        input[10] = 375;
        input[11] = 3323;
        (int256[3] memory scores, uint8 action) = model.predict(input);
        require(scores[0] == -706, "score-0");
        require(scores[1] == 3492, "score-1");
        require(scores[2] == 4841, "score-2");
        require(action == 2, "action");
    }
    function testRejectOutOfBoundInput() public {
        int32[12] memory input;
        input[0] = 16385;
        (bool ok,) = address(model).call(abi.encodeCall(model.predict, (input)));
        require(!ok, "out of bound accepted");
    }
}

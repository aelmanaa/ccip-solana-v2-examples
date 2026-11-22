// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import { Client } from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";
import { Test } from "forge-std/Test.sol";

import { PingPongPeer } from "../src/PingPongPeer.sol";

/// @dev Minimal router: quotes a flat fee and records the last message.
contract MockRouter {
    uint256 public constant FEE = 1e15;
    uint256 public sends;
    uint64 public lastDest;
    bytes public lastData;
    bytes public lastReceiver;

    function getFee(uint64, Client.EVM2AnyMessage memory) external pure returns (uint256) {
        return FEE;
    }

    function ccipSend(uint64 dest, Client.EVM2AnyMessage calldata message) external payable returns (bytes32) {
        require(msg.value == FEE, "fee");
        ++sends;
        lastDest = dest;
        lastData = message.data;
        lastReceiver = message.receiver;
        return keccak256(abi.encode(sends));
    }

    function deliver(PingPongPeer peer, Client.Any2EVMMessage memory message) external {
        peer.ccipReceive(message);
    }
}

contract PingPongPeerTest is Test {
    uint64 internal constant SOLANA = 16423721717087811551;
    bytes internal constant PROGRAM = hex"0101010101010101010101010101010101010101010101010101010101010101";
    bytes internal constant SIGNER = hex"0202020202020202020202020202020202020202020202020202020202020202";

    MockRouter internal router;
    PingPongPeer internal peer;

    function setUp() public {
        router = new MockRouter();
        peer = new PingPongPeer(address(router));
        peer.setPeer(SOLANA, PROGRAM, SIGNER, hex"a69dd4aa");
        vm.deal(address(peer), 1 ether);
    }

    function _incoming(bytes memory sender, uint8 kind, uint64 seq)
        internal
        pure
        returns (Client.Any2EVMMessage memory)
    {
        return Client.Any2EVMMessage({
            messageId: keccak256(abi.encode(kind, seq)),
            sourceChainSelector: SOLANA,
            sender: sender,
            data: abi.encode(kind, seq),
            destTokenAmounts: new Client.EVMTokenAmount[](0)
        });
    }

    function test_PingSendsToThePeerProgram() public {
        bytes32 id = peer.ping(SOLANA);
        assertEq(id, keccak256(abi.encode(uint256(1))));
        assertEq(router.sends(), 1);
        assertEq(router.lastReceiver(), PROGRAM);
        assertEq(router.lastData(), abi.encode(uint8(1), uint64(1)));
    }

    function test_PingFromPeerIsAnsweredWithPong() public {
        router.deliver(peer, _incoming(SIGNER, 1, 7));
        assertEq(peer.pingsReceived(), 1);
        assertEq(peer.pongsSent(), 1);
        assertEq(router.lastData(), abi.encode(uint8(2), uint64(7)));
    }

    function test_PongIsRecorded() public {
        router.deliver(peer, _incoming(SIGNER, 2, 3));
        assertEq(peer.pongsReceived(), 1);
        assertEq(router.sends(), 0);
    }

    function test_OnlyThePeerSignerIsAccepted() public {
        vm.expectRevert(abi.encodeWithSelector(PingPongPeer.NotPeer.selector, SOLANA, PROGRAM));
        router.deliver(peer, _incoming(PROGRAM, 1, 1));
    }

    function test_OnlyTheRouterCanDeliver() public {
        vm.expectRevert();
        peer.ccipReceive(_incoming(SIGNER, 1, 1));
    }

    function test_OnlyOwnerPings() public {
        vm.prank(address(0xBEEF));
        vm.expectRevert();
        bytes32 id = peer.ping(SOLANA);
        assertEq(id, bytes32(0));
    }
}

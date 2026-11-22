// SPDX-License-Identifier: MIT
pragma solidity 0.8.24;

import { CCIPReceiver } from "@chainlink/contracts-ccip/contracts/applications/CCIPReceiver.sol";
import { IRouterClient } from "@chainlink/contracts-ccip/contracts/interfaces/IRouterClient.sol";
import { Client } from "@chainlink/contracts-ccip/contracts/libraries/Client.sol";
import { Ownable } from "@openzeppelin/contracts@5.3.0/access/Ownable.sol";

/// @title PingPongPeer
/// @notice EVM side of the ping-pong example. One peer per remote chain: messages go to it, and
/// only it is accepted as a sender. A ping is answered with a pong from inside `ccipReceive`,
/// paid in native ETH from this contract's balance.
/// @dev Payload: `abi.encode(uint8 kind, uint64 sequence)`, kind 1 = ping, 2 = pong.
contract PingPongPeer is CCIPReceiver, Ownable {
    uint8 public constant KIND_PING = 1;
    uint8 public constant KIND_PONG = 2;

    struct PeerConfig {
        /// Receiver bytes on the remote chain: the program id for a Solana peer.
        bytes peer;
        /// The only sender bytes accepted from that chain. For a Solana peer this is the program's
        /// `ccip_signer` PDA, which signs its sends, not the program id.
        bytes peerSender;
        /// Extra args for messages to the peer (GenericExtraArgsV3).
        bytes extraArgs;
    }

    struct CcvPolicy {
        address[] requiredCCVs;
        address[] optionalCCVs;
        uint8 optionalThreshold;
        bytes4 allowedFinalityConfig;
    }

    mapping(uint64 chainSelector => PeerConfig config) private s_peers;
    mapping(uint64 sourceChainSelector => CcvPolicy policy) private s_ccvPolicies;

    uint64 public pingsSent;
    uint64 public pingsReceived;
    uint64 public pongsSent;
    uint64 public pongsReceived;
    bytes32 public lastMessageId;
    bytes32 public lastPongMessageId;

    event PeerSet(uint64 indexed chainSelector, bytes peer, bytes peerSender, bytes extraArgs);
    event CcvPolicySet(uint64 indexed sourceChainSelector);
    event PingSent(uint64 indexed destChainSelector, uint64 sequence);
    event PongSent(uint64 indexed destChainSelector, uint64 sequence);
    event PingReceived(uint64 indexed sourceChainSelector, uint64 sequence, bytes32 messageId);
    event PongReceived(uint64 indexed sourceChainSelector, uint64 sequence, bytes32 messageId);

    error UnknownPeer(uint64 chainSelector);
    error NotPeer(uint64 sourceChainSelector, bytes sender);
    error InvalidKind(uint8 kind);
    error InsufficientBalance(uint256 balance, uint256 fee);
    error WithdrawFailed();
    error ZeroAddress();

    constructor(address router) CCIPReceiver(router) Ownable(msg.sender) { }

    receive() external payable { }

    /// @notice Sets the peer for a chain. `peer` empty removes it.
    function setPeer(uint64 chainSelector, bytes calldata peer, bytes calldata peerSender, bytes calldata extraArgs)
        external
        onlyOwner
    {
        s_peers[chainSelector] = PeerConfig({ peer: peer, peerSender: peerSender, extraArgs: extraArgs });
        emit PeerSet(chainSelector, peer, peerSender, extraArgs);
    }

    function getPeer(uint64 chainSelector) external view returns (PeerConfig memory) {
        return s_peers[chainSelector];
    }

    /// @notice Sets the CCVs and finality this contract requires for messages from a chain.
    function setCcvPolicy(uint64 sourceChainSelector, CcvPolicy calldata policy) external onlyOwner {
        s_ccvPolicies[sourceChainSelector] = policy;
        emit CcvPolicySet(sourceChainSelector);
    }

    /// @inheritdoc CCIPReceiver
    function getCCVsAndFinalityConfig(uint64 sourceChainSelector, bytes calldata)
        external
        view
        override
        returns (
            address[] memory requiredCCVs,
            address[] memory optionalCCVs,
            uint8 optionalThreshold,
            bytes4 allowedFinalityConfig
        )
    {
        CcvPolicy storage p = s_ccvPolicies[sourceChainSelector];
        return (p.requiredCCVs, p.optionalCCVs, p.optionalThreshold, p.allowedFinalityConfig);
    }

    /// @notice Fee for one message to the peer of `chainSelector`, in wei.
    function quote(uint64 chainSelector) external view returns (uint256) {
        return IRouterClient(getRouter()).getFee(chainSelector, _message(chainSelector, KIND_PING, 0));
    }

    /// @notice Sends a ping to the peer of `destChainSelector`, paid from this contract's balance.
    /// The message id is in the router's `CCIPMessageSent` event of the same transaction.
    function ping(uint64 destChainSelector) external onlyOwner returns (bytes32) {
        uint64 sequence = ++pingsSent;
        emit PingSent(destChainSelector, sequence);
        (Client.EVM2AnyMessage memory message, uint256 fee) = _prepare(destChainSelector, KIND_PING, sequence);
        return IRouterClient(getRouter()).ccipSend{ value: fee }(destChainSelector, message);
    }

    function withdraw(address payable to, uint256 amount) external onlyOwner {
        if (to == address(0)) revert ZeroAddress();
        (bool ok,) = to.call{ value: amount }("");
        if (!ok) revert WithdrawFailed();
    }

    function _ccipReceive(Client.Any2EVMMessage memory message) internal override {
        uint64 source = message.sourceChainSelector;
        bytes memory expected = s_peers[source].peerSender;
        if (expected.length == 0 || keccak256(expected) != keccak256(message.sender)) {
            revert NotPeer(source, message.sender);
        }
        (uint8 kind, uint64 sequence) = abi.decode(message.data, (uint8, uint64));
        lastMessageId = message.messageId;
        if (kind == KIND_PONG) {
            ++pongsReceived;
            emit PongReceived(source, sequence, message.messageId);
            return;
        }
        if (kind != KIND_PING) revert InvalidKind(kind);
        ++pingsReceived;
        ++pongsSent;
        emit PingReceived(source, sequence, message.messageId);
        emit PongSent(source, sequence);
        (Client.EVM2AnyMessage memory pong, uint256 fee) = _prepare(source, KIND_PONG, sequence);
        lastPongMessageId = IRouterClient(getRouter()).ccipSend{ value: fee }(source, pong);
    }

    function _prepare(uint64 chainSelector, uint8 kind, uint64 sequence)
        private
        view
        returns (Client.EVM2AnyMessage memory message, uint256 fee)
    {
        message = _message(chainSelector, kind, sequence);
        fee = IRouterClient(getRouter()).getFee(chainSelector, message);
        if (address(this).balance < fee) revert InsufficientBalance(address(this).balance, fee);
    }

    function _message(uint64 chainSelector, uint8 kind, uint64 sequence)
        private
        view
        returns (Client.EVM2AnyMessage memory)
    {
        PeerConfig storage p = s_peers[chainSelector];
        if (p.peer.length == 0) revert UnknownPeer(chainSelector);
        return Client.EVM2AnyMessage({
            receiver: p.peer,
            data: abi.encode(kind, sequence),
            tokenAmounts: new Client.EVMTokenAmount[](0),
            feeToken: address(0),
            extraArgs: p.extraArgs
        });
    }
}

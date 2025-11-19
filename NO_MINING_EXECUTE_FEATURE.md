# No-Mining Execute Mode

## Overview

This patch adds a new execution mode to Anvil that allows transactions to execute immediately and update state without mining blocks. This mode is only available when `--no-mining` is enabled.

## Motivation

In the original `--no-mining` mode:
- Transactions are added to the pool but not executed
- State doesn't change until blocks are manually mined via `evm_mine`
- This can be limiting for certain testing scenarios

The new `--no-mining-execute` mode allows:
- Transactions to execute immediately upon submission
- State changes to be applied (balances, storage, contract deployments, etc.)
- Transaction receipts to be created and queryable
- No blocks to be created
- Block number to remain unchanged

## Usage

```bash
anvil --no-mining --no-mining-execute
```

**Note:** The `--no-mining-execute` flag requires `--no-mining` to be set.
for new block and new block timestamp, user need to set manually via `evm_mine` and `evm_setNextBlockTimestamp` like:
```
    w3.provider.make_request('evm_setNextBlockTimestamp', [w3.eth.get_block('latest')['timestamp'] + 12])
    w3.provider.make_request('evm_mine', [])
```


## Implementation Details

### Files Modified

1. **crates/anvil/src/cmd.rs**
   - Added `--no-mining-execute` CLI flag
   - Flag requires `--no-mining` to be set
   - Wired up to NodeConfig

2. **crates/anvil/src/config.rs**
   - Added `no_mining_execute: bool` field to `NodeConfig`
   - Added builder method `with_no_mining_execute()`
   - Updated Default implementation

3. **crates/anvil/src/eth/backend/mem/mod.rs**
   - Added `execute_transaction_without_mining()` method
   - This method executes a transaction and commits state changes without creating a block
   - Creates and stores transaction receipts so they can be queried via `eth_getTransactionReceipt`
   - Added `node_config()` accessor method

4. **crates/anvil/src/eth/api.rs**
   - Modified `send_raw_transaction()` to check for no-mining-execute mode
   - Modified `add_pending_transaction()` to execute immediately when enabled
   - Both methods now execute transactions immediately if the mode is active

### How It Works

1. Transaction is submitted via `eth_sendRawTransaction` or `eth_sendUnsignedTransaction`
2. Transaction is added to the pool (as normal)
3. If `no_mining` && `no_mining_execute` are both true:
   - `execute_transaction_without_mining()` is called
   - Transaction is executed using `TransactionExecutor`
   - State changes are committed to the database
   - Transaction receipt is created and stored
   - No block is created
   - Block number doesn't increment
4. Transaction hash is returned to the caller
5. Receipt can be queried via `eth_getTransactionReceipt` or `wait_for_transaction_receipt()`

### Key Differences from Other Modes

| Mode | Executes TX | Creates Block | Updates State | Creates Receipt | Block Number |
|------|-------------|---------------|---------------|-----------------|--------------|
| Default (instant) | ✅ Immediate | ✅ Yes | ✅ Yes | ✅ Yes | Increments |
| `--no-mining` | ❌ No | ❌ No | ❌ No | ❌ No | Unchanged |
| `--no-mining --no-mining-execute` | ✅ Immediate | ❌ No | ✅ Yes | ✅ Yes | Unchanged |
| `--block-time N` | ✅ Interval | ✅ Yes | ✅ Yes | ✅ Yes | Increments |

## Testing

To test the implementation:

1. Build Anvil:
   ```bash
   cargo build -p anvil --release
   ```

2. Run Anvil with the new mode:
   ```bash
   ./target/release/anvil --no-mining --no-mining-execute
   ```

3. Send a transaction:
   ```bash
   cast send --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
     --value 1ether \
     0x70997970C51812dc3A010C7d01b50e0d17dc79C8
   ```

4. Verify state changed:
   ```bash
   cast balance 0x70997970C51812dc3A010C7d01b50e0d17dc79C8
   # Should show increased balance
   ```

5. Verify no block was mined:
   ```bash
   cast block-number
   # Should still be 0
   ```

6. Or use the provided Python test script:
   ```bash
   python3 test_receipt.py
   ```

### Test Results

The feature has been successfully tested and verified:

```
✅ Connected to Anvil
📦 Initial block number: 0
💰 Initial balance of recipient: 10000 ETH

🚀 Sending transaction...
⏳ Waiting for transaction receipt...
✅ Receipt received!
   - Block number: 0
   - Gas used: 21000
   - Status: 1

💰 Final balance of recipient: 10001 ETH
📦 Final block number: 0
✅ Block number unchanged (as expected)
✅ Balance updated correctly
✅ All tests passed!
```

**Key Verification Points:**
- ✅ `wait_for_transaction_receipt()` returns immediately (no hanging)
- ✅ Transaction receipts are queryable via `eth_getTransactionReceipt`
- ✅ State changes are applied (balances updated)
- ✅ Block number remains unchanged
- ✅ No new blocks are created

## Use Cases

This mode is useful for:
- Testing state changes without advancing block numbers
- Simulating multiple transactions at the same block height
- Testing scenarios where block progression needs to be controlled separately from transaction execution
- Performance testing where block creation overhead should be avoided


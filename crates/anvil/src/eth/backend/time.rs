//! Manages the block time

use crate::eth::error::BlockchainError;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::{sync::Arc, time::Duration};

/// Returns the `Utc` datetime for the given seconds since unix epoch
pub fn utc_from_secs(secs: u64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs as i64, 0).unwrap()
}

/// Manages block time
#[derive(Clone, Debug)]
pub struct TimeManager {
    /// tracks the overall applied timestamp offset
    offset: Arc<RwLock<i128>>,
    /// The timestamp of the last finalized block header
    last_timestamp: Arc<RwLock<u64>>,
    /// The timestamp of the last virtual block (used in --no-mining-execute mode)
    /// This is used for monotonic timestamp enforcement across virtual blocks
    last_virtual_timestamp: Arc<RwLock<u64>>,
    /// Contains the next timestamp to use
    /// if this is set then the next time `[TimeManager::current_timestamp()]` is called this value
    /// will be taken and returned. After which the `offset` will be updated accordingly
    next_exact_timestamp: Arc<RwLock<Option<u64>>>,
    /// The interval to use when determining the next block's timestamp
    interval: Arc<RwLock<Option<u64>>>,
}

impl TimeManager {
    pub fn new(start_timestamp: u64) -> Self {
        let time_manager = Self {
            last_timestamp: Default::default(),
            last_virtual_timestamp: Default::default(),
            offset: Default::default(),
            next_exact_timestamp: Default::default(),
            interval: Default::default(),
        };
        time_manager.reset(start_timestamp);
        time_manager
    }

    /// Resets the current time manager to the given timestamp, resetting the offsets and
    /// next block timestamp option
    pub fn reset(&self, start_timestamp: u64) {
        let current = duration_since_unix_epoch().as_secs() as i128;
        *self.last_timestamp.write() = start_timestamp;
        *self.last_virtual_timestamp.write() = start_timestamp;
        *self.offset.write() = (start_timestamp as i128) - current;
        self.next_exact_timestamp.write().take();
    }

    pub fn offset(&self) -> i128 {
        *self.offset.read()
    }

    /// Adds the given `offset` to the already tracked offset and returns the result
    fn add_offset(&self, offset: i128) -> i128 {
        let mut current = self.offset.write();
        let next = current.saturating_add(offset);
        trace!(target: "time", "adding timestamp offset={}, total={}", offset, next);
        *current = next;
        next
    }

    /// Jumps forward in time by the given seconds
    ///
    /// This will apply a permanent offset to the natural UNIX Epoch timestamp
    pub fn increase_time(&self, seconds: u64) -> i128 {
        self.add_offset(seconds as i128)
    }

    /// Sets the exact timestamp to use in the next block
    /// Fails if it's before (or at the same time) the last timestamp
    pub fn set_next_block_timestamp(&self, timestamp: u64) -> Result<(), BlockchainError> {
        trace!(target: "time", "override next timestamp {}", timestamp);
        if timestamp < *self.last_timestamp.read() {
            return Err(BlockchainError::TimestampError(format!(
                "{timestamp} is lower than previous block's timestamp"
            )));
        }
        self.next_exact_timestamp.write().replace(timestamp);
        Ok(())
    }

    /// Sets an interval to use when computing the next timestamp
    ///
    /// If an interval already exists, this will update the interval, otherwise a new interval will
    /// be set starting with the current timestamp.
    pub fn set_block_timestamp_interval(&self, interval: u64) {
        trace!(target: "time", "set interval {}", interval);
        self.interval.write().replace(interval);
    }

    /// Removes the interval if it exists
    pub fn remove_block_timestamp_interval(&self) -> bool {
        if self.interval.write().take().is_some() {
            trace!(target: "time", "removed interval");
            true
        } else {
            false
        }
    }

    /// Computes the next timestamp without updating internals
    /// Uses last_virtual_timestamp for monotonic enforcement to support virtual blocks
    fn compute_next_timestamp(&self) -> (u64, Option<i128>) {
        let current = duration_since_unix_epoch().as_secs() as i128;
        let last_timestamp = *self.last_timestamp.read();
        let last_virtual_timestamp = *self.last_virtual_timestamp.read();

        let next_exact = *self.next_exact_timestamp.read();
        trace!(target: "time", "compute_next_timestamp: current={}, last={}, last_virtual={}, next_exact={:?}",
               current, last_timestamp, last_virtual_timestamp, next_exact);

        let mut next_timestamp =
            if let Some(next) = next_exact {
                // When using evm_setNextBlockTimestamp, use the exact timestamp
                // but don't update the offset (it should only be used once)
                trace!(target: "time", "Using next_exact_timestamp: {}", next);
                next
            } else if let Some(interval) = *self.interval.read() {
                last_timestamp.saturating_add(interval)
            } else {
                current.saturating_add(self.offset()) as u64
            };
        // Ensures that the timestamp is always increasing
        // Use last_virtual_timestamp for the check to support virtual blocks in --no-mining-execute mode
        if next_timestamp < last_virtual_timestamp {
            next_timestamp = last_virtual_timestamp + 1;
        }
        trace!(target: "time", "Computed next_timestamp: {}", next_timestamp);
        // Don't update offset when using evm_setNextBlockTimestamp
        // The offset should only be updated by evm_increaseTime or evm_setTime
        (next_timestamp, None)
    }

    /// Returns the current timestamp and updates the underlying offset and interval accordingly
    pub fn next_timestamp(&self) -> u64 {
        let (next_timestamp, next_offset) = self.compute_next_timestamp();
        // Make sure we reset the `next_exact_timestamp`
        let cleared = self.next_exact_timestamp.write().take();
        trace!(target: "time", "next_timestamp: returning {}, cleared next_exact={:?}", next_timestamp, cleared);
        if let Some(next_offset) = next_offset {
            *self.offset.write() = next_offset;
        }
        *self.last_timestamp.write() = next_timestamp;
        next_timestamp
    }

    /// Returns the next timestamp for a virtual block without updating last_timestamp.
    /// This is used in --no-mining-execute mode where blocks are not immediately finalized.
    /// The last_timestamp should only be updated when blocks are actually mined/finalized.
    ///
    /// When next_exact_timestamp is set (via evm_setNextBlockTimestamp), all virtual transactions
    /// will use that same timestamp until the block is mined. This allows multiple transactions
    /// to execute with the same timestamp.
    pub fn next_virtual_timestamp(&self) -> u64 {
        let (next_timestamp, next_offset) = self.compute_next_timestamp();

        // DO NOT clear next_exact_timestamp here - it should persist across virtual transactions
        // until the block is finalized. This allows all transactions in a virtual block to have
        // the same timestamp when evm_setNextBlockTimestamp is used.
        let has_exact = self.next_exact_timestamp.read().is_some();
        trace!(target: "time", "next_virtual_timestamp: returning {}, has_exact={}", next_timestamp, has_exact);

        if let Some(next_offset) = next_offset {
            *self.offset.write() = next_offset;
        }

        // Only update last_virtual_timestamp if we're NOT using next_exact_timestamp
        // When using next_exact_timestamp, all virtual txs should have the same timestamp
        if !has_exact {
            *self.last_virtual_timestamp.write() = next_timestamp;
        }

        // DO NOT update last_timestamp - virtual blocks are not finalized yet
        next_timestamp
    }

    /// Returns the current timestamp for a call that does _not_ update the value
    pub fn current_call_timestamp(&self) -> u64 {
        let (next_timestamp, _) = self.compute_next_timestamp();
        next_timestamp
    }

    /// Sets the last timestamp to the given value.
    /// This is used when finalizing virtual blocks in --no-mining-execute mode.
    /// Also updates last_virtual_timestamp to keep them in sync.
    pub fn set_last_timestamp(&self, timestamp: u64) {
        trace!(target: "time", "set_last_timestamp: {}", timestamp);
        *self.last_timestamp.write() = timestamp;
        *self.last_virtual_timestamp.write() = timestamp;
    }

    /// Peeks at the next exact timestamp without consuming it
    pub fn peek_next_exact_timestamp(&self) -> Option<u64> {
        *self.next_exact_timestamp.read()
    }

    /// Clears the next exact timestamp
    pub fn clear_next_exact_timestamp(&self) {
        self.next_exact_timestamp.write().take();
    }
}

/// Returns the current duration since unix epoch.
pub fn duration_since_unix_epoch() -> Duration {
    use std::time::SystemTime;
    let now = SystemTime::now();
    now.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_else(|err| panic!("Current time {now:?} is invalid: {err:?}"))
}

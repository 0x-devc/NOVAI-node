//! AI Resource Allocation Hooks (D18.5)
//!
//! PURPOSE: Infrastructure for AI entities to specify and track compute budgets.
//! This module provides the types and tracking mechanisms, but does NOT enforce limits.
//!
//! INVARIANTS:
//! - Budget specifications are declarative only
//! - Tracking is observational (no enforcement yet)
//! - All values use integer math for determinism
//!
//! FAILURE MODES:
//! - Over-budget tasks are logged but not prevented (enforcement is future work)
//!
//! FUTURE WORK (not this week):
//! - Budget enforcement
//! - Resource metering
//! - Cost accounting

use novai_types::Address;
use std::collections::HashMap;

/// Resource budget specification for an AI task.
///
/// AI entities can declare their expected resource consumption.
/// This is currently advisory/tracking only - enforcement is future work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceBudget {
    /// Maximum compute units for this task.
    /// Units are abstract and will be defined by the runtime.
    pub max_compute_units: u64,

    /// Maximum memory bytes for this task.
    pub max_memory_bytes: u64,

    /// Maximum storage bytes for artifacts.
    pub max_storage_bytes: u64,

    /// Priority level (0 = lowest, 255 = highest).
    /// Higher priority tasks may preempt lower priority ones (future).
    pub priority: u8,

    /// Task identifier for tracking.
    pub task_id: [u8; 32],
}

impl ResourceBudget {
    /// Create a new resource budget.
    #[must_use]
    pub fn new(task_id: [u8; 32]) -> Self {
        Self {
            max_compute_units: 0,
            max_memory_bytes: 0,
            max_storage_bytes: 0,
            priority: 128, // Default medium priority
            task_id,
        }
    }

    /// Set maximum compute units.
    #[must_use]
    pub fn with_compute(mut self, units: u64) -> Self {
        self.max_compute_units = units;
        self
    }

    /// Set maximum memory.
    #[must_use]
    pub fn with_memory(mut self, bytes: u64) -> Self {
        self.max_memory_bytes = bytes;
        self
    }

    /// Set maximum storage.
    #[must_use]
    pub fn with_storage(mut self, bytes: u64) -> Self {
        self.max_storage_bytes = bytes;
        self
    }

    /// Set priority level.
    #[must_use]
    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    /// Encode budget to bytes for on-chain storage.
    ///
    /// Format:
    /// - task_id: 32 bytes
    /// - max_compute_units: u64 LE
    /// - max_memory_bytes: u64 LE
    /// - max_storage_bytes: u64 LE
    /// - priority: u8
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(57);
        data.extend_from_slice(&self.task_id);
        data.extend_from_slice(&self.max_compute_units.to_le_bytes());
        data.extend_from_slice(&self.max_memory_bytes.to_le_bytes());
        data.extend_from_slice(&self.max_storage_bytes.to_le_bytes());
        data.push(self.priority);
        data
    }

    /// Decode budget from bytes.
    #[must_use]
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() != 57 {
            return None;
        }

        let task_id: [u8; 32] = data[0..32].try_into().ok()?;
        let max_compute_units = u64::from_le_bytes(data[32..40].try_into().ok()?);
        let max_memory_bytes = u64::from_le_bytes(data[40..48].try_into().ok()?);
        let max_storage_bytes = u64::from_le_bytes(data[48..56].try_into().ok()?);
        let priority = data[56];

        Some(Self {
            task_id,
            max_compute_units,
            max_memory_bytes,
            max_storage_bytes,
            priority,
        })
    }
}

/// Tracked resource usage for a task.
#[derive(Debug, Clone, Default)]
pub struct ResourceUsage {
    /// Compute units consumed.
    pub compute_units_used: u64,

    /// Peak memory bytes used.
    pub peak_memory_bytes: u64,

    /// Storage bytes written.
    pub storage_bytes_written: u64,

    /// Whether task exceeded its budget (logged but not enforced).
    pub exceeded_budget: bool,
}

impl ResourceUsage {
    /// Create new empty usage tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record compute unit consumption.
    pub fn record_compute(&mut self, units: u64) {
        self.compute_units_used = self.compute_units_used.saturating_add(units);
    }

    /// Record memory usage (tracks peak).
    pub fn record_memory(&mut self, bytes: u64) {
        self.peak_memory_bytes = self.peak_memory_bytes.max(bytes);
    }

    /// Record storage write.
    pub fn record_storage(&mut self, bytes: u64) {
        self.storage_bytes_written = self.storage_bytes_written.saturating_add(bytes);
    }

    /// Check if usage exceeds budget and mark if so.
    pub fn check_budget(&mut self, budget: &ResourceBudget) {
        if self.compute_units_used > budget.max_compute_units
            || self.peak_memory_bytes > budget.max_memory_bytes
            || self.storage_bytes_written > budget.max_storage_bytes
        {
            self.exceeded_budget = true;
        }
    }
}

/// Resource tracker for multiple AI entities.
///
/// This tracker is observational only - it records usage but does not
/// enforce limits. Enforcement is planned for future work.
#[derive(Debug, Default)]
pub struct ResourceTracker {
    /// Active budgets by task ID.
    budgets: HashMap<[u8; 32], ResourceBudget>,

    /// Usage tracking by task ID.
    usage: HashMap<[u8; 32], ResourceUsage>,

    /// Per-entity aggregate usage.
    entity_usage: HashMap<Address, ResourceUsage>,

    /// Total tasks tracked.
    tasks_tracked: u64,

    /// Total tasks that exceeded budget.
    tasks_exceeded: u64,
}

impl ResourceTracker {
    /// Create a new resource tracker.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a budget for a task.
    pub fn register_budget(&mut self, budget: ResourceBudget) {
        let task_id = budget.task_id;
        self.budgets.insert(task_id, budget);
        self.usage.insert(task_id, ResourceUsage::new());
        self.tasks_tracked += 1;
    }

    /// Record compute usage for a task.
    pub fn record_compute(&mut self, task_id: &[u8; 32], units: u64) {
        if let Some(usage) = self.usage.get_mut(task_id) {
            usage.record_compute(units);
            self.check_and_log_exceeded(task_id);
        }
    }

    /// Record memory usage for a task.
    pub fn record_memory(&mut self, task_id: &[u8; 32], bytes: u64) {
        if let Some(usage) = self.usage.get_mut(task_id) {
            usage.record_memory(bytes);
            self.check_and_log_exceeded(task_id);
        }
    }

    /// Record storage usage for a task.
    pub fn record_storage(&mut self, task_id: &[u8; 32], bytes: u64) {
        if let Some(usage) = self.usage.get_mut(task_id) {
            usage.record_storage(bytes);
            self.check_and_log_exceeded(task_id);
        }
    }

    /// Check if task exceeded budget and log if so.
    fn check_and_log_exceeded(&mut self, task_id: &[u8; 32]) {
        if let (Some(usage), Some(budget)) =
            (self.usage.get_mut(task_id), self.budgets.get(task_id))
        {
            let was_exceeded = usage.exceeded_budget;
            usage.check_budget(budget);

            // Count first-time exceeds
            if !was_exceeded && usage.exceeded_budget {
                self.tasks_exceeded += 1;
                // NOTE: In future, this would trigger enforcement.
                // Currently just tracked for monitoring.
            }
        }
    }

    /// Get usage for a task.
    #[must_use]
    pub fn get_usage(&self, task_id: &[u8; 32]) -> Option<&ResourceUsage> {
        self.usage.get(task_id)
    }

    /// Get budget for a task.
    #[must_use]
    pub fn get_budget(&self, task_id: &[u8; 32]) -> Option<&ResourceBudget> {
        self.budgets.get(task_id)
    }

    /// Complete a task and aggregate usage to entity.
    pub fn complete_task(&mut self, task_id: &[u8; 32], entity: Address) {
        if let Some(usage) = self.usage.remove(task_id) {
            let entity_usage = self.entity_usage.entry(entity).or_default();
            entity_usage.compute_units_used += usage.compute_units_used;
            entity_usage.storage_bytes_written += usage.storage_bytes_written;
            entity_usage.peak_memory_bytes =
                entity_usage.peak_memory_bytes.max(usage.peak_memory_bytes);
            if usage.exceeded_budget {
                entity_usage.exceeded_budget = true;
            }
        }
        self.budgets.remove(task_id);
    }

    /// Get aggregate usage for an entity.
    #[must_use]
    pub fn get_entity_usage(&self, entity: &Address) -> Option<&ResourceUsage> {
        self.entity_usage.get(entity)
    }

    /// Get total tasks tracked.
    #[must_use]
    pub fn tasks_tracked(&self) -> u64 {
        self.tasks_tracked
    }

    /// Get count of tasks that exceeded budget.
    #[must_use]
    pub fn tasks_exceeded(&self) -> u64 {
        self.tasks_exceeded
    }

    /// Get count of currently active tasks.
    #[must_use]
    pub fn active_tasks(&self) -> usize {
        self.usage.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_task_id(seed: u8) -> [u8; 32] {
        [seed; 32]
    }

    #[test]
    fn budget_builder_pattern() {
        let budget = ResourceBudget::new(test_task_id(1))
            .with_compute(1000)
            .with_memory(1024 * 1024)
            .with_storage(4096)
            .with_priority(200);

        assert_eq!(budget.max_compute_units, 1000);
        assert_eq!(budget.max_memory_bytes, 1024 * 1024);
        assert_eq!(budget.max_storage_bytes, 4096);
        assert_eq!(budget.priority, 200);
    }

    #[test]
    fn budget_encode_decode_roundtrip() {
        let budget = ResourceBudget::new(test_task_id(42))
            .with_compute(5000)
            .with_memory(2048)
            .with_storage(512)
            .with_priority(100);

        let encoded = budget.encode();
        assert_eq!(encoded.len(), 57);

        let decoded = ResourceBudget::decode(&encoded).expect("decode");
        assert_eq!(budget, decoded);
    }

    #[test]
    fn budget_decode_wrong_length_fails() {
        assert!(ResourceBudget::decode(&[0u8; 56]).is_none());
        assert!(ResourceBudget::decode(&[0u8; 58]).is_none());
        assert!(ResourceBudget::decode(&[]).is_none());
    }

    #[test]
    fn usage_tracking() {
        let mut usage = ResourceUsage::new();

        usage.record_compute(100);
        usage.record_compute(50);
        assert_eq!(usage.compute_units_used, 150);

        usage.record_memory(1000);
        usage.record_memory(500);
        assert_eq!(usage.peak_memory_bytes, 1000); // Peak, not sum

        usage.record_storage(200);
        usage.record_storage(300);
        assert_eq!(usage.storage_bytes_written, 500);
    }

    #[test]
    fn usage_exceeds_budget() {
        let budget = ResourceBudget::new(test_task_id(1)).with_compute(100);

        let mut usage = ResourceUsage::new();
        usage.record_compute(50);
        usage.check_budget(&budget);
        assert!(!usage.exceeded_budget);

        usage.record_compute(60); // Now at 110, over 100
        usage.check_budget(&budget);
        assert!(usage.exceeded_budget);
    }

    #[test]
    fn tracker_registers_and_tracks() {
        let mut tracker = ResourceTracker::new();
        let task_id = test_task_id(1);

        let budget = ResourceBudget::new(task_id).with_compute(1000);
        tracker.register_budget(budget);

        assert_eq!(tracker.tasks_tracked(), 1);
        assert_eq!(tracker.active_tasks(), 1);

        tracker.record_compute(&task_id, 500);
        let usage = tracker.get_usage(&task_id).unwrap();
        assert_eq!(usage.compute_units_used, 500);
    }

    #[test]
    fn tracker_detects_exceeded() {
        let mut tracker = ResourceTracker::new();
        let task_id = test_task_id(1);

        let budget = ResourceBudget::new(task_id).with_compute(100);
        tracker.register_budget(budget);

        tracker.record_compute(&task_id, 150); // Over budget

        assert_eq!(tracker.tasks_exceeded(), 1);
        let usage = tracker.get_usage(&task_id).unwrap();
        assert!(usage.exceeded_budget);
    }

    #[test]
    fn tracker_completes_task() {
        let mut tracker = ResourceTracker::new();
        let task_id = test_task_id(1);
        let entity: Address = [0x42; 32];

        let budget = ResourceBudget::new(task_id).with_compute(1000);
        tracker.register_budget(budget);
        tracker.record_compute(&task_id, 500);

        assert_eq!(tracker.active_tasks(), 1);

        tracker.complete_task(&task_id, entity);

        assert_eq!(tracker.active_tasks(), 0);
        assert!(tracker.get_usage(&task_id).is_none());

        let entity_usage = tracker.get_entity_usage(&entity).unwrap();
        assert_eq!(entity_usage.compute_units_used, 500);
    }

    #[test]
    fn tracker_aggregates_entity_usage() {
        let mut tracker = ResourceTracker::new();
        let entity: Address = [0x42; 32];

        // Task 1
        let task1 = test_task_id(1);
        tracker.register_budget(ResourceBudget::new(task1).with_compute(1000));
        tracker.record_compute(&task1, 300);
        tracker.complete_task(&task1, entity);

        // Task 2
        let task2 = test_task_id(2);
        tracker.register_budget(ResourceBudget::new(task2).with_compute(1000));
        tracker.record_compute(&task2, 200);
        tracker.complete_task(&task2, entity);

        let entity_usage = tracker.get_entity_usage(&entity).unwrap();
        assert_eq!(entity_usage.compute_units_used, 500); // 300 + 200
    }

    #[test]
    fn tracker_is_observational_only() {
        // This test documents that the tracker is observational.
        // It does NOT:
        // - Prevent over-budget operations
        // - Reject tasks that exceed limits
        // - Enforce any resource constraints
        //
        // It ONLY:
        // - Records budgets
        // - Tracks usage
        // - Flags exceeded budgets
        //
        // Enforcement is planned for future work.

        let mut tracker = ResourceTracker::new();
        let task_id = test_task_id(1);

        let budget = ResourceBudget::new(task_id).with_compute(100);
        tracker.register_budget(budget);

        // Record usage that exceeds budget - this is NOT prevented
        tracker.record_compute(&task_id, 500);

        // Task is flagged but NOT stopped
        let usage = tracker.get_usage(&task_id).unwrap();
        assert!(usage.exceeded_budget);
        assert_eq!(usage.compute_units_used, 500); // Still recorded

        // INVARIANT: Tracker is observational only - no enforcement.
    }
}

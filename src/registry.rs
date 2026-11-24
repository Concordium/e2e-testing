//! Ordered registration and outcomes shared by suite-specific executors.

use crate::report::{Check, Outcome};

/// A suite-specific test definition paired with its report state.
#[derive(Debug)]
pub struct Registered<T> {
    /// Callable or lifecycle implementation owned by the suite.
    pub definition: T,
    /// Display metadata and outcome updated during execution.
    pub check: Check,
}

/// Owns test definitions and outcomes in registration order.
#[derive(Debug)]
pub struct TestRegistry<T> {
    entries: Vec<Registered<T>>,
}

impl<T> Default for TestRegistry<T> {
    fn default() -> Self {
        Self {
            entries: Default::default(),
        }
    }
}

/// Registers tests under one display heading.
#[derive(Debug)]
pub struct TestGroup<'a, T> {
    registry: &'a mut TestRegistry<T>,
    name: &'static str,
}

impl<T> TestRegistry<T> {
    /// Open a registration group without adding a report entry.
    pub fn group(&mut self, name: &'static str) -> TestGroup<'_, T> {
        TestGroup {
            registry: self,
            name,
        }
    }

    /// Read test definitions and outcomes in registration order.
    pub fn entries(&self) -> &[Registered<T>] {
        &self.entries
    }

    /// Access definitions and outcomes for suite-specific execution.
    pub fn entries_mut(&mut self) -> &mut [Registered<T>] {
        &mut self.entries
    }

    /// Consume the registry, drop definitions, and return ordered report checks.
    pub fn into_checks(self) -> Vec<Check> {
        self.entries.into_iter().map(|entry| entry.check).collect()
    }
}

impl<T> TestGroup<'_, T> {
    /// Append a test with a NotRun outcome. Return this group for chaining.
    pub fn register(&mut self, name: impl Into<String>, definition: T) -> &mut Self {
        self.registry.entries.push(Registered {
            definition,
            check: Check {
                group: self.name.into(),
                name: name.into(),
                outcome: Outcome::NotRun,
            },
        });
        self
    }
}

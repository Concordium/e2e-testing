//! Registration of P11 token admin-role scenarios.

mod allow_and_deny_lists;
mod assign_and_revoke_admin_roles;
mod disabled_features;
mod execution_budget;
mod initial_admin_roles;
mod invalid_admin_operations;
mod operational_roles;
mod paused_administration;
mod role_isolation_and_sequencing;
mod support;

use super::Test;
use crate::registry::TestRegistry;

pub(super) fn register(registry: &mut TestRegistry<Test>) {
    let mut group = registry.group("P11 Token Admin Roles");
    initial_admin_roles::register(&mut group);
    assign_and_revoke_admin_roles::register(&mut group);
    execution_budget::register(&mut group);
    disabled_features::register(&mut group);
    invalid_admin_operations::register(&mut group);
    operational_roles::register(&mut group);
    allow_and_deny_lists::register(&mut group);
    paused_administration::register(&mut group);
    role_isolation_and_sequencing::register(&mut group);
}

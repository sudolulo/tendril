//! `tendrild` — the Tendril orchestrator daemon entry point.

use tendril_orchestrator::Role;

fn main() {
    // Phase 1 will parse config/flags to select the role and start the control loop.
    let role = Role::SingleNode;
    println!("tendrild starting (role: {role:?}) — scaffold; control loop lands in Phase 1.");
}

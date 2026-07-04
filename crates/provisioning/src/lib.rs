//! Host provisioning strategies.
//!
//! A [`strategy::ProvisioningStrategy`] turns a GPU's assessed capability into the host changes
//! required to use it (VFIO binding today; vGPU `mdev`/SR-IOV later). Changes are expressed as bootc
//! image layers so they inherit atomic rollback.

pub mod apply;
pub mod passthrough;
pub mod strategy;

pub use apply::{Action, Mode};
pub use passthrough::PassthroughStrategy;
pub use strategy::{ProvisioningPlan, ProvisioningStrategy};

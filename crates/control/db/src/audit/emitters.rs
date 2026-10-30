//! Linked production audit emission sites, independent of catalogue declarations and tests.

/// One action value used by a compiled production writer.
#[derive(Debug, Clone, Copy)]
pub struct Emission {
    /// The exact public action name.
    pub action: &'static str,
    /// The production module that supplies it to an audit writer.
    pub module: &'static str,
}

inventory::collect!(Emission);

/// The registry implementation used by the emission macros.
#[doc(hidden)]
pub use inventory as registry;

/// Every linked production emission site; an action may have several writers.
pub fn registered() -> impl Iterator<Item = &'static Emission> {
    inventory::iter::<Emission>.into_iter()
}

/// Registers and returns the typed action supplied at an actual production audit call.
///
/// Test builds of the invoking module cannot contribute registrations. Use this in
/// the writer's action expression, never in the action catalogue or a coverage list.
#[macro_export]
macro_rules! audit_action {
    ($action:path) => {{
        #[cfg(not(test))]
        $crate::audit::emitters::registry::submit! {
            $crate::audit::emitters::Emission {
                action: $action.as_str(),
                module: module_path!(),
            }
        }
        $action
    }};
}

/// Registers and returns an exact action name supplied at a production audit call.
///
/// Like the typed form, test-only calls contribute no catalogue coverage.
#[macro_export]
macro_rules! audit_name {
    ($action:literal) => {{
        #[cfg(not(test))]
        $crate::audit::emitters::registry::submit! {
            $crate::audit::emitters::Emission {
                action: $action,
                module: module_path!(),
            }
        }
        $action
    }};
}

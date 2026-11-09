//! Typed assembly links between the shared API implementation and its route families.

use crate::registry::Registry;
use std::sync::OnceLock;

/// The stable facade supplies exactly one complete set of route families.
pub struct Assembly {
    pub initialize: fn(),
    pub mount: fn(&mut Registry),
}
inventory::collect!(Assembly);

static INITIALIZED: OnceLock<()> = OnceLock::new();

/// Installing function addresses does not retain a request, caller, clock or connection.
pub fn initialize() {
    INITIALIZED.get_or_init(|| {
        let mut assemblies = inventory::iter::<Assembly>.into_iter();
        let assembly = assemblies.next();
        assert!(
            assembly.is_some(),
            "the API facade must link its route assembly"
        );
        assert!(
            assemblies.next().is_none(),
            "only one API facade can be linked"
        );
        if let Some(assembly) = assembly {
            (assembly.initialize)();
        }
    });
}

/// Native test/application builders use the same facade assembly as the executable.
pub fn mount(registry: &mut Registry) {
    initialize();
    for assembly in inventory::iter::<Assembly> {
        (assembly.mount)(registry);
    }
}

#[doc(hidden)]
pub fn callback<T: Copy + 'static>(slot: &'static OnceLock<T>, name: &'static str) -> T {
    initialize();
    match slot.get() {
        Some(callback) => *callback,
        None => unreachable!("the assembled API family did not install {name}"),
    }
}

/// An interface names the existing native arguments and result; no serialized callback exists.
#[doc(hidden)]
#[macro_export]
macro_rules! api_family_callback {
    (@store [$(#[$cfg:meta])*] $handle:ident, $slot:ident, $setter:ident, $callback:ty) => {
        $(#[$cfg])*
        #[doc(hidden)]
        pub type $handle = $callback;
        $(#[$cfg])*
        static $slot: ::std::sync::OnceLock<$handle> = ::std::sync::OnceLock::new();
        $(#[$cfg])*
        #[doc(hidden)]
        pub fn $setter(callback: $handle) {
            let installed = *$slot.get_or_init(|| callback);
            assert!(::std::ptr::fn_addr_eq(installed, callback),
                "an API family callback cannot be replaced after assembly");
        }
    };
    (
        bind $binding:ident, $installer:ident, $shim:ident;
        slot $handle:ident, $slot:ident, $setter:ident;
        base $base:path;
        $(#[$cfg:meta])*
        async fn $name:ident<$lt:lifetime>($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;
    ) => {
        $crate::api_family_callback!(@store [$(#[$cfg])*] $handle, $slot, $setter,
            for<$lt> fn($($ty),*) -> ::futures::future::BoxFuture<$lt, $ret>);
        $(#[$cfg])*
        pub async fn $name<$lt>($($arg: $ty),*) -> $ret {
            ($crate::route_assembly::callback(&$slot, concat!(module_path!(), "::", stringify!($name))))($($arg),*).await
        }
        $(#[$cfg])*
        #[doc(hidden)]
        #[macro_export]
        macro_rules! $binding {
            () => {
                fn $shim<$lt>($($arg: $ty),*) -> ::futures::future::BoxFuture<$lt, $ret> {
                    ::std::boxed::Box::pin(self::$name($($arg),*))
                }
                #[doc(hidden)]
                pub fn $installer() {
                    use $base as __api_base_module;
                    __api_base_module::$setter(self::$shim);
                }
            };
        }
    };
    (
        bind $binding:ident, $installer:ident, $shim:ident;
        slot $handle:ident, $slot:ident, $setter:ident;
        base $base:path;
        $(#[$cfg:meta])*
        async fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;
    ) => {
        $crate::api_family_callback!(@store [$(#[$cfg])*] $handle, $slot, $setter,
            fn($($ty),*) -> ::futures::future::BoxFuture<'static, $ret>);
        $(#[$cfg])*
        pub async fn $name($($arg: $ty),*) -> $ret {
            ($crate::route_assembly::callback(&$slot, concat!(module_path!(), "::", stringify!($name))))($($arg),*).await
        }
        $(#[$cfg])*
        #[doc(hidden)]
        #[macro_export]
        macro_rules! $binding {
            () => {
                fn $shim($($arg: $ty),*) -> ::futures::future::BoxFuture<'static, $ret> {
                    ::std::boxed::Box::pin(self::$name($($arg),*))
                }
                #[doc(hidden)]
                pub fn $installer() {
                    use $base as __api_base_module;
                    __api_base_module::$setter(self::$shim);
                }
            };
        }
    };
    (
        bind $binding:ident, $installer:ident, $shim:ident;
        slot $handle:ident, $slot:ident, $setter:ident;
        base $base:path;
        $(#[$cfg:meta])*
        fn $name:ident<$lt:lifetime>($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;
    ) => {
        $crate::api_family_callback!(@store [$(#[$cfg])*] $handle, $slot, $setter,
            for<$lt> fn($($ty),*) -> $ret);
        $(#[$cfg])*
        pub fn $name<$lt>($($arg: $ty),*) -> $ret {
            ($crate::route_assembly::callback(&$slot, concat!(module_path!(), "::", stringify!($name))))($($arg),*)
        }
        $(#[$cfg])*
        #[doc(hidden)]
        #[macro_export]
        macro_rules! $binding {
            () => {
                #[doc(hidden)]
                pub fn $installer() {
                    use $base as __api_base_module;
                    __api_base_module::$setter(self::$name);
                }
            };
        }
    };
    (
        bind $binding:ident, $installer:ident, $shim:ident;
        slot $handle:ident, $slot:ident, $setter:ident;
        base $base:path;
        $(#[$cfg:meta])*
        fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;
    ) => {
        $crate::api_family_callback!(@store [$(#[$cfg])*] $handle, $slot, $setter,
            fn($($ty),*) -> $ret);
        $(#[$cfg])*
        pub fn $name($($arg: $ty),*) -> $ret {
            ($crate::route_assembly::callback(&$slot, concat!(module_path!(), "::", stringify!($name))))($($arg),*)
        }
        $(#[$cfg])*
        #[doc(hidden)]
        #[macro_export]
        macro_rules! $binding {
            () => {
                #[doc(hidden)]
                pub fn $installer() {
                    use $base as __api_base_module;
                    __api_base_module::$setter(self::$name);
                }
            };
        }
    };
}

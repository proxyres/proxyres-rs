//! Functions resolved from libraries loaded at run-time.

/// Declares a struct of functions resolved by name from a library, which it keeps loaded.
macro_rules! dylib_api {
    (
        $vis:vis struct $name:ident {
            $(fn $symbol:ident($($arg:ty),* $(,)?) $(-> $ret:ty)?;)*
        }
    ) => {
        #[allow(non_snake_case)]
        $vis struct $name {
            // The library must outlive the functions resolved from it
            _library: libloading::Library,
            $($symbol: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl $name {
            /// Resolves each function from the library by its name.
            ///
            /// # Safety
            ///
            /// Each function must have the signature declared for it.
            unsafe fn resolve(library: libloading::Library) -> Option<Self> {
                // SAFETY: The caller guarantees the signatures.
                unsafe {
                    Some($name {
                        $($symbol: *library.get(concat!(stringify!($symbol), "\0").as_bytes()).ok()?,)*
                        _library: library,
                    })
                }
            }
        }
    };
}

pub(crate) use dylib_api;

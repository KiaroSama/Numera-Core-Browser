mod node;
mod run;
mod scenarios;

use napi_sys as sys;
use node::{Context, Js};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Once,
};

static INIT: Once = Once::new();

/// Node calls this entry point when loading the native fixture module.
///
/// # Safety
/// Node supplies a live N-API environment and a valid module exports handle.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_register_module_v1(env: sys::napi_env, exports: Js) -> Js {
    // Keep the process symbol table loaded for the life of the addon.
    INIT.call_once(|| {
        // SAFETY: The addon was loaded by the Node runtime that exports N-API.
        std::mem::forget(unsafe { sys::setup() });
    });
    let ctx = Context::new(env);
    match catch_unwind(AssertUnwindSafe(|| run::start(ctx))) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("{error}");
            ctx.exit_code();
        }
        Err(_) => {
            eprintln!("SDK adapter failed to initialize");
            ctx.exit_code();
        }
    }
    exports
}

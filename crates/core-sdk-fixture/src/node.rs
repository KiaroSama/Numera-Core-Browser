//! Node handles are valid only for the current callback. Keep values across
//! promises through N-API references and release them before the run ends.
#![allow(unsafe_code)]

use napi_sys as sys;
use serde_json::Value;
use std::{
    ffi::{CString, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
};

pub type Js = sys::napi_value;
pub type Ref = sys::napi_ref;
pub type Result<T> = std::result::Result<T, String>;
type Job = Box<dyn FnOnce(Context, Js)>;

#[derive(Clone, Copy)]
pub struct Context(sys::napi_env);

struct Callback(Option<Job>);

macro_rules! call {
    ($name:ident($($arg:expr),* $(,)?)) => {{
        // SAFETY: Context is supplied by Node for an active callback. Each
        // wrapper creates live input buffers and output slots for this call.
        unsafe { sys::$name($($arg),*) }
    }};
}

impl Context {
    // The module entry point receives the environment for the active Node call.
    pub(super) fn new(env: sys::napi_env) -> Self {
        Self(env)
    }

    fn check(self, status: sys::napi_status) -> Result<()> {
        if status == sys::Status::napi_ok {
            return Ok(());
        }
        if status == sys::Status::napi_pending_exception {
            let mut thrown = ptr::null_mut();
            if call!(napi_get_and_clear_last_exception(self.0, &mut thrown)) == sys::Status::napi_ok
            {
                return Err(self.error_text(thrown));
            }
        }
        Err(format!("Node-API call failed ({status})"))
    }

    fn value(self, function: impl FnOnce(*mut Js) -> sys::napi_status) -> Result<Js> {
        let mut result = ptr::null_mut();
        self.check(function(&mut result))?;
        Ok(result)
    }

    pub fn global(self) -> Result<Js> {
        self.value(|out| call!(napi_get_global(self.0, out)))
    }

    pub fn undefined(self) -> Result<Js> {
        self.value(|out| call!(napi_get_undefined(self.0, out)))
    }

    pub fn process(self) -> Result<Js> {
        self.get(self.global()?, "process")
    }

    pub fn get(self, obj: Js, property: &str) -> Result<Js> {
        let property = CString::new(property).map_err(|error| error.to_string())?;
        self.value(|out| call!(napi_get_named_property(self.0, obj, property.as_ptr(), out)))
    }

    pub fn element(self, array: Js, index: u32) -> Result<Js> {
        self.value(|out| call!(napi_get_element(self.0, array, index, out)))
    }

    pub fn array_len(self, array: Js) -> Result<u32> {
        let mut size = 0;
        self.check(call!(napi_get_array_length(self.0, array, &mut size)))?;
        Ok(size)
    }

    pub fn call(self, obj: Js, method: &str, args: &[Js]) -> Result<Js> {
        self.invoke(obj, self.get(obj, method)?, args)
            .map_err(|error| format!("{method}: {error}"))
    }

    pub fn invoke(self, receiver: Js, function: Js, args: &[Js]) -> Result<Js> {
        self.value(|out| {
            call!(napi_call_function(
                self.0,
                receiver,
                function,
                args.len(),
                args.as_ptr(),
                out
            ))
        })
    }

    pub fn construct(self, ctor: Js, args: &[Js]) -> Result<Js> {
        self.value(|out| {
            call!(napi_new_instance(
                self.0,
                ctor,
                args.len(),
                args.as_ptr(),
                out
            ))
        })
    }

    pub fn string(self, value: &str) -> Result<Js> {
        let length = isize::try_from(value.len()).map_err(|error| error.to_string())?;
        self.value(|out| {
            call!(napi_create_string_utf8(
                self.0,
                value.as_ptr().cast(),
                length,
                out
            ))
        })
    }

    pub fn text(self, value: Js) -> Result<String> {
        let mut size = 0;
        self.check(call!(napi_get_value_string_utf8(
            self.0,
            value,
            ptr::null_mut(),
            0,
            &mut size
        )))?;
        if size > 16 * 1024 * 1024 {
            return Err("Node string exceeded the fixture size limit".into());
        }
        let mut bytes = vec![0; size + 1];
        self.check(call!(napi_get_value_string_utf8(
            self.0,
            value,
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            &mut size
        )))?;
        bytes.truncate(size);
        String::from_utf8(bytes).map_err(|error| error.to_string())
    }

    pub fn json(self, data: &Value) -> Result<Js> {
        let json = self.get(self.global()?, "JSON")?;
        self.call(json, "parse", &[self.string(&data.to_string())?])
    }

    pub fn read_json(self, object: Js) -> Result<Value> {
        let json = self.get(self.global()?, "JSON")?;
        let text = self.text(self.call(json, "stringify", &[object])?)?;
        serde_json::from_str(&text).map_err(|error| error.to_string())
    }

    pub fn dom_function(self, parameter: Option<&str>, source: &str) -> Result<Js> {
        let constructor = self.get(self.global()?, "Function")?;
        let source = self.string(source)?;
        if let Some(parameter) = parameter {
            self.construct(constructor, &[self.string(parameter)?, source])
        } else {
            self.construct(constructor, &[source])
        }
    }

    pub fn require(self, anchor: &str) -> Result<Js> {
        let process = self.process()?;
        let module = self.call(process, "getBuiltinModule", &[self.string("module")?])?;
        self.call(module, "createRequire", &[self.string(anchor)?])
    }

    pub fn screenshot(self, image: Js) -> Result<()> {
        let mut is_buffer = false;
        self.check(call!(napi_is_buffer(self.0, image, &mut is_buffer)))?;
        let mut bytes: *mut c_void = ptr::null_mut();
        let mut size = 0;
        if is_buffer {
            self.check(call!(napi_get_buffer_info(
                self.0, image, &mut bytes, &mut size
            )))?;
        } else {
            let mut kind = sys::TypedarrayType::uint8_array;
            self.check(call!(napi_get_typedarray_info(
                self.0,
                image,
                &mut kind,
                &mut size,
                &mut bytes,
                ptr::null_mut(),
                ptr::null_mut()
            )))?;
            if kind != sys::TypedarrayType::uint8_array {
                return Err("Expected browser screenshot bytes".into());
            }
        }
        if bytes.is_null() || size <= 8 {
            return Err("Expected a browser screenshot in PNG format".into());
        }
        // SAFETY: Node returned a live view into the screenshot's backing
        // buffer, and the byte length was checked above.
        let header = unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), 8) };
        check_png(header)
    }

    pub fn reference(self, object: Js) -> Result<Ref> {
        let mut reference = ptr::null_mut();
        self.check(call!(napi_create_reference(
            self.0,
            object,
            1,
            &mut reference
        )))?;
        Ok(reference)
    }

    pub fn dereference(self, reference: Ref) -> Result<Js> {
        self.value(|out| call!(napi_get_reference_value(self.0, reference, out)))
    }

    pub fn release(self, reference: Ref) {
        let _ = call!(napi_delete_reference(self.0, reference));
    }

    pub fn then(self, value: Js, success: Job, failure: Job) -> Result<()> {
        let promise = self.get(self.global()?, "Promise")?;
        let promise = self.call(promise, "resolve", &[value])?;
        let success = self.callback(success)?;
        let failure = self.callback(failure)?;
        self.call(promise, "then", &[success, failure])?;
        Ok(())
    }

    fn callback(self, job: Job) -> Result<Js> {
        let data = Box::into_raw(Box::new(Callback(Some(job))));
        let label = c"numeraSdkStep";
        let result = self.value(|out| {
            call!(napi_create_function(
                self.0,
                label.as_ptr(),
                label.to_bytes().len() as isize,
                Some(callback),
                data.cast(),
                out
            ))
        });
        let result = result.and_then(|function| {
            self.check(call!(napi_add_finalizer(
                self.0,
                function,
                data.cast(),
                Some(finalize),
                ptr::null_mut(),
                ptr::null_mut()
            )))?;
            Ok(function)
        });
        if result.is_err() {
            // SAFETY: The function has not been handed to JavaScript and has no
            // registered finalizer. This restores the Box's sole ownership.
            drop(unsafe { Box::from_raw(data) });
        }
        result
    }

    pub fn error_text(self, value: Js) -> String {
        let mut text = ptr::null_mut();
        if call!(napi_coerce_to_string(self.0, value, &mut text)) != sys::Status::napi_ok {
            return "JavaScript rejected the SDK operation".into();
        }
        self.text(text)
            .unwrap_or_else(|_| "JavaScript rejected the SDK operation".into())
    }

    pub fn exit_code(self) {
        if let Ok((process, one)) = self.process().and_then(|process| {
            self.value(|out| call!(napi_create_int32(self.0, 1, out)))
                .map(|one| (process, one))
        }) {
            let _ = call!(napi_set_named_property(
                self.0,
                process,
                c"exitCode".as_ptr(),
                one
            ));
        }
    }
}

pub fn check_png(header: &[u8]) -> Result<()> {
    if header.get(..8) == Some(b"\x89PNG\r\n\x1a\n") {
        Ok(())
    } else {
        Err("Expected a browser screenshot in PNG format".into())
    }
}

unsafe extern "C" fn callback(env: sys::napi_env, info: sys::napi_callback_info) -> Js {
    let ctx = Context::new(env);
    let mut data = ptr::null_mut();
    let mut argc = 1;
    let mut args = [ptr::null_mut(); 1];
    let status = call!(napi_get_cb_info(
        env,
        info,
        &mut argc,
        args.as_mut_ptr(),
        ptr::null_mut(),
        &mut data
    ));
    if status == sys::Status::napi_ok && !data.is_null() && argc == 1 {
        // SAFETY: The callback owns this pointer until Node calls finalize.
        let data = unsafe { &mut *data.cast::<Callback>() };
        if let Some(job) = data.0.take()
            && catch_unwind(AssertUnwindSafe(|| job(ctx, args[0]))).is_err()
        {
            eprintln!("SDK adapter callback panicked");
            ctx.exit_code();
        }
    }
    ctx.undefined().unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn finalize(_: sys::napi_env, data: *mut c_void, _: *mut c_void) {
    if !data.is_null() {
        // SAFETY: This pointer came from Box::into_raw and this finalizer owns it.
        drop(unsafe { Box::from_raw(data.cast::<Callback>()) });
    }
}

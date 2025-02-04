//! Defines WasmEdge Function and FuncType structs.

use crate::{
    error::{FuncError, WasmEdgeError},
    ffi, BoxedFn, Engine, WasmEdgeResult, WasmValue, HOST_FUNCS,
};
use core::ffi::c_void;
use rand::Rng;
use std::convert::TryInto;
use wasmedge_types::ValType;

extern "C" fn wraper_fn(
    key_ptr: *mut c_void,
    _data: *mut c_void,
    _mem_cxt: *mut ffi::WasmEdge_MemoryInstanceContext,
    params: *const ffi::WasmEdge_Value,
    param_len: u32,
    returns: *mut ffi::WasmEdge_Value,
    return_len: u32,
) -> ffi::WasmEdge_Result {
    let key = key_ptr as *const usize as usize;

    let input = {
        let raw_input = unsafe {
            std::slice::from_raw_parts(
                params,
                param_len
                    .try_into()
                    .expect("len of params should not greater than usize"),
            )
        };
        raw_input.iter().map(|r| (*r).into()).collect::<Vec<_>>()
    };

    let return_len = return_len
        .try_into()
        .expect("len of returns should not greater than usize");
    let raw_returns = unsafe { std::slice::from_raw_parts_mut(returns, return_len) };

    let result = {
        let host_functions = HOST_FUNCS.lock().expect("[wasmedge-sys] try lock failed.");
        let real_fn = host_functions
            .get(&key)
            .expect("host function should be there");
        real_fn(input)
    };

    match result {
        Ok(v) => {
            assert!(v.len() == return_len);
            for (idx, item) in v.into_iter().enumerate() {
                raw_returns[idx] = item.as_raw();
            }
            ffi::WasmEdge_Result { Code: 0 }
        }
        Err(c) => ffi::WasmEdge_Result { Code: c as u8 },
    }
}

/// Defines a host function.
///
/// A WasmEdge [Function] defines a WebAssembly host function described by its [type](crate::FuncType). A host function is a closure of the original function defined in either the host or the WebAssembly module.
///
/// # Usage
///
/// To invoke a host function, `wasmedge-sys` provides two different ways:
///
/// * [Vm](crate::Vm) provides the [run_function](crate::Vm::run_function) and [run_registered_function](crate::Vm::run_registered_function) APIs to call a host function (registered into Vm) by given the name of the target host function.
///
/// * If the target host function instance and an [Executor](crate::Executor) instance are available, then there are two APIs available:
///     * Use the [run_func](crate::Engine::run_func) API defined in [Engine](crate::Engine) trait. [Executor](crate::Executor) implements the trait.
///
///     * Use the [call](crate::Function::call) API of [Function](crate::Function).
///
#[derive(Debug)]
pub struct Function {
    pub(crate) inner: InnerFunc,
    pub(crate) registered: bool,
}
impl Function {
    #[allow(clippy::type_complexity)]
    /// Creates a [host function](crate::Function) with the given function type.
    ///
    /// # Arguments
    ///
    /// * `ty` - The types of the arguments and returns of the target function.
    ///
    /// * `real_fn` - The pointer to the target function.
    ///
    /// * `cost` - The function cost in the [Statistics](crate::Statistics). Pass 0 if the calculation is not needed.
    ///
    /// # Error
    ///
    /// If fail to create a [Function], then an error is returned.
    ///
    /// # Example
    ///
    /// The example defines a host function `real_add`, and creates a [Function] binding to it by calling
    /// the `create_binding` method.
    ///
    /// ```rust
    /// use wasmedge_sys::{FuncType, Function, WasmValue};
    /// use wasmedge_types::{ValType, WasmEdgeResult};
    ///
    /// fn real_add(inputs: Vec<WasmValue>) -> Result<Vec<WasmValue>, u8> {
    ///     if inputs.len() != 2 {
    ///         return Err(1);
    ///     }
    ///
    ///     let a = if inputs[0].ty() == ValType::I32 {
    ///         inputs[0].to_i32()
    ///     } else {
    ///         return Err(2);
    ///     };
    ///
    ///     let b = if inputs[1].ty() == ValType::I32 {
    ///         inputs[1].to_i32()
    ///     } else {
    ///         return Err(3);
    ///     };
    ///
    ///     let c = a + b;
    ///
    ///     Ok(vec![WasmValue::from_i32(c)])
    /// }
    ///
    /// // create a FuncType
    /// let func_ty = FuncType::create(vec![ValType::I32; 2], vec![ValType::I32]).expect("fail to create a FuncType");
    ///
    /// // create a Function instance
    /// let func = Function::create(&func_ty, Box::new(real_add), 0).expect("fail to create a Function instance");
    /// ```
    pub fn create(ty: &FuncType, real_fn: BoxedFn, cost: u64) -> WasmEdgeResult<Self> {
        let mut host_functions = HOST_FUNCS.lock().expect("[wasmedge-sys] try lock failed.");
        if host_functions.len() >= host_functions.capacity() {
            return Err(WasmEdgeError::Func(FuncError::CreateBinding(format!(
                "The number of the host functions reaches the upper bound: {}",
                host_functions.capacity()
            ))));
        }

        // generate key for the coming host function
        let mut rng = rand::thread_rng();
        let mut key: usize = rng.gen();
        while host_functions.contains_key(&key) {
            key = rng.gen();
        }
        host_functions.insert(key, real_fn);

        let ctx = unsafe {
            ffi::WasmEdge_FunctionInstanceCreateBinding(
                ty.inner.0,
                Some(wraper_fn),
                key as *const usize as *mut c_void,
                std::ptr::null_mut(),
                cost,
            )
        };

        match ctx.is_null() {
            true => Err(WasmEdgeError::Func(FuncError::Create)),
            false => Ok(Self {
                inner: InnerFunc(ctx),
                registered: false,
            }),
        }
    }

    /// Returns the underlying wasm type of this [Function].
    ///
    /// # Errors
    ///
    /// If fail to get the function type, then an error is returned.
    ///
    pub fn ty(&self) -> WasmEdgeResult<FuncType> {
        let ty = unsafe { ffi::WasmEdge_FunctionInstanceGetFunctionType(self.inner.0) };
        match ty.is_null() {
            true => Err(WasmEdgeError::Func(FuncError::Type)),
            false => Ok(FuncType {
                inner: InnerFuncType(ty as *mut _),
                registered: true,
            }),
        }
    }

    /// Runs this host function and returns the result.
    ///
    /// # Arguments
    ///
    /// * `engine` - The object implements Engine trait.
    ///
    /// * `args` - The arguments passed to the host function.
    ///
    /// # Error
    ///
    /// If fail to run the host function, then an error is returned.
    ///
    /// # Example
    ///
    /// ```rust
    /// use wasmedge_sys::{FuncType, Function, WasmValue, Executor};
    /// use wasmedge_types::ValType;
    ///
    /// fn real_add(input: Vec<WasmValue>) -> Result<Vec<WasmValue>, u8> {
    ///     println!("Rust: Entering Rust function real_add");
    ///
    ///     if input.len() != 2 {
    ///         return Err(1);
    ///     }
    ///
    ///     let a = if input[0].ty() == ValType::I32 {
    ///         input[0].to_i32()
    ///     } else {
    ///         return Err(2);
    ///     };
    ///
    ///     let b = if input[1].ty() == ValType::I32 {
    ///         input[1].to_i32()
    ///     } else {
    ///         return Err(3);
    ///     };
    ///
    ///     let c = a + b;
    ///     println!("Rust: calcuating in real_add c: {:?}", c);
    ///
    ///     println!("Rust: Leaving Rust function real_add");
    ///     Ok(vec![WasmValue::from_i32(c)])
    /// }
    ///
    /// // create a FuncType
    /// let result = FuncType::create(vec![ValType::I32; 2], vec![ValType::I32]);
    /// assert!(result.is_ok());
    /// let func_ty = result.unwrap();
    /// // create a host function
    /// let result = Function::create(&func_ty, Box::new(real_add), 0);
    /// assert!(result.is_ok());
    /// let host_func = result.unwrap();
    ///
    /// // create an Executor instance
    /// let result = Executor::create(None, None);
    /// assert!(result.is_ok());
    /// let mut executor = result.unwrap();
    ///
    /// // run this function
    /// let result = host_func.call(
    ///     &mut executor,
    ///     vec![WasmValue::from_i32(1), WasmValue::from_i32(2)],
    /// );
    /// assert!(result.is_ok());
    /// let returns = result.unwrap();
    /// assert_eq!(returns[0].to_i32(), 3);
    /// ```
    pub fn call<E: Engine>(
        &self,
        engine: &mut E,
        args: impl IntoIterator<Item = WasmValue>,
    ) -> WasmEdgeResult<Vec<WasmValue>> {
        engine.run_func(self, args)
    }

    /// Returns a reference to this [Function] instance.
    pub fn as_ref(&self) -> FuncRef {
        FuncRef {
            inner: InnerFuncRef(self.inner.0 as *const _),
        }
    }
}
impl Drop for Function {
    fn drop(&mut self) {
        if !self.registered && !self.inner.0.is_null() {
            unsafe { ffi::WasmEdge_FunctionInstanceDelete(self.inner.0) };
        }
    }
}

#[derive(Debug)]
pub(crate) struct InnerFunc(pub(crate) *mut ffi::WasmEdge_FunctionInstanceContext);
unsafe impl Send for InnerFunc {}
unsafe impl Sync for InnerFunc {}

/// Defines the type of a [host function](crate::Function).
///
/// A WasmEdge [FuncType] classifies the signature of a [Function], including the type information of both the arguments and the returns.
#[derive(Debug)]
pub struct FuncType {
    pub(crate) inner: InnerFuncType,
    pub(crate) registered: bool,
}
impl FuncType {
    /// Create a new [FuncType] to be associated with the given arguments and returns.
    ///
    /// # Arguments
    ///
    /// * `args` - The agrgument types of a [Function].
    ///
    /// * `returns` - The types of the returns of a [Function].
    ///
    /// # Error
    ///
    /// If fail to create a [FuncType], then an error is returned.
    ///
    /// # Example
    ///
    /// ```rust
    /// use wasmedge_sys::FuncType;
    /// use wasmedge_types::ValType;
    ///
    /// let func_ty = FuncType::create(vec![ValType::I32;2], vec![ValType::I32]).expect("fail to create a FuncType");
    /// ```
    pub fn create<I: IntoIterator<Item = ValType>, R: IntoIterator<Item = ValType>>(
        args: I,
        returns: R,
    ) -> WasmEdgeResult<Self> {
        let param_tys = args
            .into_iter()
            .map(|x| x.into())
            .collect::<Vec<ffi::WasmEdge_ValType>>();
        let ret_tys = returns
            .into_iter()
            .map(|x| x.into())
            .collect::<Vec<ffi::WasmEdge_ValType>>();

        let ctx = unsafe {
            ffi::WasmEdge_FunctionTypeCreate(
                param_tys.as_ptr() as *const _,
                param_tys.len() as u32,
                ret_tys.as_ptr() as *const _,
                ret_tys.len() as u32,
            )
        };
        match ctx.is_null() {
            true => Err(WasmEdgeError::FuncTypeCreate),
            false => Ok(Self {
                inner: InnerFuncType(ctx),
                registered: false,
            }),
        }
    }

    /// Returns the number of the arguments of a [Function].
    pub fn params_len(&self) -> u32 {
        unsafe { ffi::WasmEdge_FunctionTypeGetParametersLength(self.inner.0) }
    }

    /// Returns an Iterator of the arguments of a [Function].
    pub fn params_type_iter(&self) -> impl Iterator<Item = ValType> {
        let len = self.params_len();
        let mut types = Vec::with_capacity(len as usize);
        unsafe {
            ffi::WasmEdge_FunctionTypeGetParameters(self.inner.0, types.as_mut_ptr(), len);
            types.set_len(len as usize);
        }

        types.into_iter().map(Into::into)
    }

    ///Returns the number of the returns of a [Function].
    pub fn returns_len(&self) -> u32 {
        unsafe { ffi::WasmEdge_FunctionTypeGetReturnsLength(self.inner.0) }
    }

    /// Returns an Iterator of the return types of a [Function].
    pub fn returns_type_iter(&self) -> impl Iterator<Item = ValType> {
        let len = self.returns_len();
        let mut types = Vec::with_capacity(len as usize);
        unsafe {
            ffi::WasmEdge_FunctionTypeGetReturns(self.inner.0, types.as_mut_ptr(), len);
            types.set_len(len as usize);
        }

        types.into_iter().map(Into::into)
    }
}
impl Drop for FuncType {
    fn drop(&mut self) {
        if !self.registered && !self.inner.0.is_null() {
            unsafe { ffi::WasmEdge_FunctionTypeDelete(self.inner.0) };
        }
    }
}
impl From<wasmedge_types::FuncType> for FuncType {
    fn from(ty: wasmedge_types::FuncType) -> Self {
        let param_tys: Vec<_> = match ty.args() {
            Some(args) => args.to_vec(),
            None => Vec::new(),
        };
        let ret_tys: Vec<_> = match ty.returns() {
            Some(returns) => returns.to_vec(),
            None => Vec::new(),
        };

        FuncType::create(param_tys, ret_tys).expect("[wasmedge-sys] Failed to convert wasmedge_types::FuncType into wasmedge_sys::FuncType.")
    }
}
impl From<FuncType> for wasmedge_types::FuncType {
    fn from(ty: FuncType) -> Self {
        let args = if ty.params_len() > 0 {
            let mut args = Vec::with_capacity(ty.params_len() as usize);
            for ty in ty.params_type_iter() {
                args.push(ty);
            }
            Some(args)
        } else {
            None
        };

        let returns = if ty.returns_len() > 0 {
            let mut returns = Vec::with_capacity(ty.returns_len() as usize);
            for ty in ty.returns_type_iter() {
                returns.push(ty);
            }
            Some(returns)
        } else {
            None
        };

        wasmedge_types::FuncType::new(args, returns)
    }
}

#[derive(Debug)]
pub(crate) struct InnerFuncType(pub(crate) *mut ffi::WasmEdge_FunctionTypeContext);
unsafe impl Send for InnerFuncType {}
unsafe impl Sync for InnerFuncType {}

/// Defines a reference to a [host function](crate::Function).
#[derive(Debug, Clone)]
pub struct FuncRef {
    pub(crate) inner: InnerFuncRef,
}
impl Drop for FuncRef {
    fn drop(&mut self) {
        self.inner.0 = std::ptr::null();
    }
}
impl FuncRef {
    /// Returns the underlying wasm type of the host function this [FuncRef] points to.
    ///
    /// # Errors
    ///
    /// If fail to get the function type, then an error is returned.
    ///
    pub fn ty(&self) -> WasmEdgeResult<FuncType> {
        let ty = unsafe { ffi::WasmEdge_FunctionInstanceGetFunctionType(self.inner.0 as *mut _) };
        match ty.is_null() {
            true => Err(WasmEdgeError::Func(FuncError::Type)),
            false => Ok(FuncType {
                inner: InnerFuncType(ty as *mut _),
                registered: true,
            }),
        }
    }

    /// Runs this host function the reference refers to.
    ///
    /// # Arguments
    ///
    /// * `engine` - The object implements Engine trait.
    ///
    /// * `args` - The arguments passed to the host function.
    ///
    /// # Error
    ///
    /// If fail to run the host function, then an error is returned.
    ///
    pub fn call<E: Engine>(
        &self,
        engine: &mut E,
        args: impl IntoIterator<Item = WasmValue>,
    ) -> WasmEdgeResult<Vec<WasmValue>> {
        engine.run_func_ref(self, args)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InnerFuncRef(pub(crate) *const ffi::WasmEdge_FunctionInstanceContext);
unsafe impl Send for InnerFuncRef {}
unsafe impl Sync for InnerFuncRef {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{types::WasmValue, Executor};
    use std::{
        sync::{Arc, Mutex},
        thread,
    };
    use wasmedge_types::ValType;

    #[test]
    fn test_func_type() {
        // test FuncType with args and returns
        {
            let param_tys = vec![
                ValType::I32,
                ValType::I64,
                ValType::F32,
                ValType::F64,
                ValType::V128,
                ValType::ExternRef,
            ];
            let param_len = param_tys.len();
            let ret_tys = vec![ValType::FuncRef, ValType::ExternRef, ValType::V128];
            let ret_len = ret_tys.len();

            // create FuncType
            let result = FuncType::create(param_tys, ret_tys);
            assert!(result.is_ok());
            let func_ty = result.unwrap();

            // check parameters
            assert_eq!(func_ty.params_len(), param_len as u32);
            let param_tys = func_ty.params_type_iter().collect::<Vec<_>>();
            assert_eq!(
                param_tys,
                vec![
                    ValType::I32,
                    ValType::I64,
                    ValType::F32,
                    ValType::F64,
                    ValType::V128,
                    ValType::ExternRef,
                ]
            );

            // check returns
            assert_eq!(func_ty.returns_len(), ret_len as u32);
            let return_tys = func_ty.returns_type_iter().collect::<Vec<_>>();
            assert_eq!(
                return_tys,
                vec![ValType::FuncRef, ValType::ExternRef, ValType::V128]
            );
        }

        // test FuncType without args and returns
        {
            // create FuncType
            let result = FuncType::create([], []);
            assert!(result.is_ok());
            let func_ty = result.unwrap();

            assert_eq!(func_ty.params_len(), 0);
            assert_eq!(func_ty.returns_len(), 0);
        }
    }

    #[test]
    fn test_func_basic() {
        // create a FuncType
        let result = FuncType::create(vec![ValType::I32; 2], vec![ValType::I32]);
        assert!(result.is_ok());
        let func_ty = result.unwrap();
        // create a host function
        let result = Function::create(&func_ty, Box::new(real_add), 0);
        assert!(result.is_ok());
        let host_func = result.unwrap();

        // get func type
        let result = host_func.ty();
        assert!(result.is_ok());
        let ty = result.unwrap();

        // check parameters
        assert_eq!(ty.params_len(), 2);
        let param_tys = ty.params_type_iter().collect::<Vec<_>>();
        assert_eq!(param_tys, vec![ValType::I32; 2]);

        // check returns
        assert_eq!(ty.returns_len(), 1);
        let return_tys = ty.returns_type_iter().collect::<Vec<_>>();
        assert_eq!(return_tys, vec![ValType::I32]);

        // run this function
        let result = Executor::create(None, None);
        assert!(result.is_ok());
        let mut executor = result.unwrap();
        let result = host_func.call(
            &mut executor,
            vec![WasmValue::from_i32(1), WasmValue::from_i32(2)],
        );
        assert!(result.is_ok());
        let returns = result.unwrap();
        assert_eq!(returns[0].to_i32(), 3);
    }

    #[test]
    fn test_func_send() {
        // create a FuncType
        let result = FuncType::create(vec![ValType::I32; 2], vec![ValType::I32]);
        assert!(result.is_ok());
        let func_ty = result.unwrap();
        // create a host function
        let result = Function::create(&func_ty, Box::new(real_add), 0);
        assert!(result.is_ok());
        let host_func = result.unwrap();

        let handle = thread::spawn(move || {
            // get func type
            let result = host_func.ty();
            assert!(result.is_ok());
            let ty = result.unwrap();

            // check parameters
            assert_eq!(ty.params_len(), 2);
            let param_tys = ty.params_type_iter().collect::<Vec<_>>();
            assert_eq!(param_tys, vec![ValType::I32; 2]);

            // check returns
            assert_eq!(ty.returns_len(), 1);
            let return_tys = ty.returns_type_iter().collect::<Vec<_>>();
            assert_eq!(return_tys, vec![ValType::I32]);
        });

        handle.join().unwrap()
    }

    #[test]
    fn test_func_sync() {
        // create a FuncType
        let result = FuncType::create(vec![ValType::I32; 2], vec![ValType::I32]);
        assert!(result.is_ok());
        let func_ty = result.unwrap();
        // create a host function
        let result = Function::create(&func_ty, Box::new(real_add), 0);
        assert!(result.is_ok());
        let host_func = Arc::new(Mutex::new(result.unwrap()));

        let host_func_cloned = Arc::clone(&host_func);
        let handle = thread::spawn(move || {
            let result = host_func_cloned.lock();
            assert!(result.is_ok());
            let host_func = result.unwrap();

            // get func type
            let result = host_func.ty();
            assert!(result.is_ok());
            let ty = result.unwrap();

            // check parameters
            assert_eq!(ty.params_len(), 2);
            let param_tys = ty.params_type_iter().collect::<Vec<_>>();
            assert_eq!(param_tys, vec![ValType::I32; 2]);

            // check returns
            assert_eq!(ty.returns_len(), 1);
            let return_tys = ty.returns_type_iter().collect::<Vec<_>>();
            assert_eq!(return_tys, vec![ValType::I32]);
        });

        handle.join().unwrap();
    }

    fn real_add(input: Vec<WasmValue>) -> Result<Vec<WasmValue>, u8> {
        println!("Rust: Entering Rust function real_add");

        if input.len() != 2 {
            return Err(1);
        }

        let a = if input[0].ty() == ValType::I32 {
            input[0].to_i32()
        } else {
            return Err(2);
        };

        let b = if input[1].ty() == ValType::I32 {
            input[1].to_i32()
        } else {
            return Err(3);
        };

        let c = a + b;
        println!("Rust: calcuating in real_add c: {:?}", c);

        println!("Rust: Leaving Rust function real_add");
        Ok(vec![WasmValue::from_i32(c)])
    }
}

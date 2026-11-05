//! Small, audited dynamic binding to Unicorn 2.1's C API.
//! The DLL/so is loaded once per engine and outlives every callback and CPU view.
use libloading::Library;
use std::{
    ffi::{CStr, c_void},
    path::PathBuf,
    ptr::NonNull,
};
pub const R0: i32 = 66;
pub const SP: i32 = 12;
pub const LR: i32 = 10;
pub const PC: i32 = 11;
pub const CPSR: i32 = 3;
pub const SPSR: i32 = 13;
type Raw = *mut c_void;
type Hook =
    unsafe extern "C" fn(Raw, *mut usize, i32, *mut c_void, *mut c_void, u64, u64, ...) -> i32;
type Ctl = unsafe extern "C" fn(Raw, u32, ...) -> i32;
pub struct Api {
    _lib: Library,
    pub close: unsafe extern "C" fn(Raw) -> i32,
    open: unsafe extern "C" fn(i32, i32, *mut Raw) -> i32,
    read: unsafe extern "C" fn(Raw, u64, *mut c_void, usize) -> i32,
    write: unsafe extern "C" fn(Raw, u64, *const c_void, usize) -> i32,
    reg_read: unsafe extern "C" fn(Raw, i32, *mut c_void) -> i32,
    reg_write: unsafe extern "C" fn(Raw, i32, *const c_void) -> i32,
    map: unsafe extern "C" fn(Raw, u64, usize, u32) -> i32,
    start: unsafe extern "C" fn(Raw, u64, u64, u64, usize) -> i32,
    stop: unsafe extern "C" fn(Raw) -> i32,
    hook: Hook,
    ctl: Ctl,
    strerror: unsafe extern "C" fn(i32) -> *const i8,
}

impl Api {
    pub fn load() -> Result<Box<Self>, String> {
        let mut paths = Vec::new();
        if let Some(p) = std::env::var_os("DSMC_UNICORN_LIBRARY") {
            paths.push(PathBuf::from(p));
        }
        let filename = if cfg!(windows) {
            "unicorn.dll"
        } else {
            "libunicorn.so.2"
        };
        if let Ok(exe) = std::env::current_exe() {
            paths.push(exe.with_file_name(filename));
        }
        paths.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("native")
                .join(if cfg!(windows) { "win64" } else { "linux" })
                .join(filename),
        );
        paths.push(PathBuf::from(filename));
        let mut errors = Vec::new();
        for path in paths {
            // SAFETY: only the documented Unicorn 2.1 C ABI is resolved here. The owning
            // Library remains in Api until all engines/callbacks have been destroyed.
            let lib = match unsafe { Library::new(&path) } {
                Ok(l) => l,
                Err(e) => {
                    errors.push(format!("{}: {e}", path.display()));
                    continue;
                }
            };
            unsafe {
                let version: unsafe extern "C" fn(*mut u32, *mut u32) -> u32 =
                    *lib.get(b"uc_version\0").map_err(|e| e.to_string())?;
                let (mut major, mut minor) = (0, 0);
                version(&mut major, &mut minor);
                if major != 2 || minor != 1 {
                    return Err(format!("Unicorn 2.1 required, found {major}.{minor}"));
                }
                macro_rules! sym {
                    ($n:literal) => {
                        *lib.get(concat!($n, "\0").as_bytes())
                            .map_err(|e| e.to_string())?
                    };
                }
                return Ok(Box::new(Self {
                    open: sym!("uc_open"),
                    close: sym!("uc_close"),
                    read: sym!("uc_mem_read"),
                    write: sym!("uc_mem_write"),
                    reg_read: sym!("uc_reg_read"),
                    reg_write: sym!("uc_reg_write"),
                    map: sym!("uc_mem_map"),
                    start: sym!("uc_emu_start"),
                    stop: sym!("uc_emu_stop"),
                    hook: sym!("uc_hook_add"),
                    ctl: sym!("uc_ctl"),
                    strerror: sym!("uc_strerror"),
                    _lib: lib,
                }));
            }
        }
        Err(format!(
            "Unicorn introuvable. Executer scripts/setup-windows.ps1.\n{}",
            errors.join("\n")
        ))
    }

    fn check(&self, code: i32) -> Result<(), String> {
        if code == 0 {
            Ok(())
        } else {
            Err(unsafe { CStr::from_ptr((self.strerror)(code)) }
                .to_string_lossy()
                .into_owned())
        }
    }

    pub fn open(&self) -> Result<Raw, String> {
        let mut p = std::ptr::null_mut();
        self.check(unsafe { (self.open)(1, 0, &mut p) })?;
        Ok(p)
    }
}

#[derive(Clone, Copy)]
pub struct Cpu<'a> {
    pub raw: Raw,
    pub api: &'a Api,
}
#[repr(C)]
struct CpReg {
    cp: u32,
    is64: u32,
    sec: u32,
    crn: u32,
    crm: u32,
    opc1: u32,
    opc2: u32,
    val: u64,
}
impl<'a> Cpu<'a> {
    pub fn new(raw: Raw, api: &'a Api) -> Self {
        Self { raw, api }
    }

    pub fn reg(self, id: i32) -> u32 {
        let mut v = 0u32;
        let e = unsafe { (self.api.reg_read)(self.raw, id, (&mut v as *mut u32).cast()) };
        assert_eq!(e, 0, "register read");
        v
    }

    pub fn set(self, id: i32, v: u32) {
        let e = unsafe { (self.api.reg_write)(self.raw, id, (&v as *const u32).cast()) };
        assert_eq!(e, 0, "register write");
    }

    pub fn cp(self, crn: u32, crm: u32, opc1: u32, opc2: u32) -> u32 {
        let mut r = CpReg {
            cp: 15,
            is64: 0,
            sec: 0,
            crn,
            crm,
            opc1,
            opc2,
            val: 0,
        };
        let e = unsafe { (self.api.reg_read)(self.raw, 139, (&mut r as *mut CpReg).cast()) };
        assert_eq!(e, 0);
        r.val as u32
    }

    pub fn set_cp(self, crn: u32, crm: u32, opc1: u32, opc2: u32, val: u32) -> Result<(), String> {
        let r = CpReg {
            cp: 15,
            is64: 0,
            sec: 0,
            crn,
            crm,
            opc1,
            opc2,
            val: val as u64,
        };
        self.api
            .check(unsafe { (self.api.reg_write)(self.raw, 139, (&r as *const CpReg).cast()) })
    }

    pub fn map(self, a: u32, n: usize) -> Result<(), String> {
        self.api
            .check(unsafe { (self.api.map)(self.raw, a as u64, n, 7) })
    }

    pub fn read(self, a: u32, n: usize) -> Result<Vec<u8>, String> {
        if n > 32 * 1024 * 1024 {
            return Err("lecture trop grande".into());
        }
        let mut v = vec![0; n];
        self.api
            .check(unsafe { (self.api.read)(self.raw, a as u64, v.as_mut_ptr().cast(), n) })?;
        Ok(v)
    }

    pub fn write(self, a: u32, b: &[u8]) -> Result<(), String> {
        self.api
            .check(unsafe { (self.api.write)(self.raw, a as u64, b.as_ptr().cast(), b.len()) })
    }

    pub fn word(self, a: u32) -> u32 {
        self.read(a, 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0)
    }

    pub fn put(self, a: u32, v: u32) {
        let _ = self.write(a, &v.to_le_bytes());
    }

    pub fn bytes(self, a: u32, values: &[u32]) {
        for (i, v) in values.iter().enumerate() {
            self.put(a + i as u32 * 4, *v)
        }
    }

    pub fn stop(self) {
        unsafe { (self.api.stop)(self.raw) };
    }

    pub fn start(self, pc: u32, count: usize, micros: u64) -> Result<(), String> {
        self.api
            .check(unsafe { (self.api.start)(self.raw, pc as u64, u32::MAX as u64, micros, count) })
    }

    pub fn cortex_a8(self) -> Result<(), String> {
        self.api
            .check(unsafe { (self.api.ctl)(self.raw, 7 | (1 << 26) | (1 << 30), 15i32) })
    }
    /// Callback/user pointers are owned by Engine and stay pinned until uc_close.
    pub unsafe fn hook(
        self,
        kind: i32,
        callback: *mut c_void,
        data: *mut c_void,
        begin: u64,
        end: u64,
    ) -> Result<(), String> {
        let mut h = 0;
        self.api
            .check(unsafe { (self.api.hook)(self.raw, &mut h, kind, callback, data, begin, end) })
    }
}

pub struct Handle {
    raw: NonNull<c_void>,
    pub api: Box<Api>,
}

impl Handle {
    pub fn new() -> Result<Self, String> {
        let api = Api::load()?;
        let raw = NonNull::new(api.open()?).ok_or("uc_open returned null")?;
        Ok(Self { raw, api })
    }

    pub fn cpu(&self) -> Cpu<'_> {
        Cpu::new(self.raw.as_ptr(), &self.api)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            (self.api.close)(self.raw.as_ptr());
        }
    }
}

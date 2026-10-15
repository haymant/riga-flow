//! GPU path propagation (Phase 9) — CUDA via `cudarc`, device code JIT-compiled with NVRTC.
//!
//! Only compiled with the `gpu` feature. Device code is compiled **to a cubin**
//! (device SASS) for the machine's exact compute capability:
//!
//! * NVRTC from this toolchain is 13.1 and emits PTX ISA **9.1**, which the
//!   installed CUDA-**13.0** driver rejects with
//!   `CUDA_ERROR_UNSUPPORTED_PTX_VERSION`. A cubin is arch-specific SASS and skips
//!   the PTX-JIT version gate entirely.
//! * Compiling device-only also sidesteps the CUDA-toolkit / glibc host-header
//!   `noexcept` conflict that a full `nvcc` host build hits on this toolchain
//!   (the same conflict the monorepo works around for llama.cpp in
//!   `scripts/riga-cuda.mjs`).
//!
//! The GPU is never a silent fallback: a `gpu` build either initialises a device
//! or fails loudly (invariants AC-10 / I-L3).

#[cfg(feature = "gpu")]
mod imp {
    use cudarc::driver::{CudaContext, CudaModule, LaunchConfig, PushKernelArg};
    use cudarc::nvrtc::Ptx;
    use riga_core::RigaError;
    use std::ffi::{CStr, CString, c_char};
    use std::sync::{Arc, OnceLock};

    /// Worst-of device kernel and the local-vol path-propagation kernel.
    const KERNEL_SRC: &str = r#"
extern "C" __global__ void worst_of(const float* a, const float* b, float* o, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) o[i] = a[i] < b[i] ? a[i] : b[i];
}

// One thread per path. `normals` are the correlated shocks [path, step, 2];
// `rows` are the per-step local-vol rows [factor, step, gx]; `ls_axis` holds each
// factor's log-strike axis (2*gx values); terminal spots are written to `out` [path, 2].
extern "C" __global__ void lv_propagate(
    const float* normals, const float* rows, const float* ls_axis,
    const float* s0, float* out,
    int paths, int steps, int gx, float dt, float rate, float q0, float q1) {
    int p = blockIdx.x * blockDim.x + threadIdx.x;
    if (p >= paths) return;
    float ls0 = logf(s0[0]);
    float ls1 = logf(s0[1]);
    float sq = sqrtf(dt);
    float lo0 = ls_axis[0];
    float inv0 = 1.0f / (ls_axis[gx - 1] - lo0);
    float lo1 = ls_axis[gx];
    float inv1 = 1.0f / (ls_axis[2 * gx - 1] - lo1);
    for (int s = 0; s < steps; ++s) {
        float x0 = (ls0 - lo0) * inv0 * (float)(gx - 1);
        if (x0 < 0.0f) x0 = 0.0f;
        if (x0 > (float)(gx - 1)) x0 = (float)(gx - 1);
        int j0 = (int)x0; if (j0 > gx - 2) j0 = gx - 2;
        float w0 = x0 - (float)j0;
        int b0 = s * gx;
        float sig0 = rows[b0 + j0] * (1.0f - w0) + rows[b0 + j0 + 1] * w0;
        float x1 = (ls1 - lo1) * inv1 * (float)(gx - 1);
        if (x1 < 0.0f) x1 = 0.0f;
        if (x1 > (float)(gx - 1)) x1 = (float)(gx - 1);
        int j1 = (int)x1; if (j1 > gx - 2) j1 = gx - 2;
        float w1 = x1 - (float)j1;
        int b1 = (steps + s) * gx;
        float sig1 = rows[b1 + j1] * (1.0f - w1) + rows[b1 + j1 + 1] * w1;
        int nb = (p * steps + s) * 2;
        ls0 += (rate - q0 - 0.5f * sig0 * sig0) * dt + sig0 * sq * normals[nb];
        ls1 += (rate - q1 - 0.5f * sig1 * sig1) * dt + sig1 * sq * normals[nb + 1];
    }
    out[p * 2] = expf(ls0);
    out[p * 2 + 1] = expf(ls1);
}
"#;

    fn gpu_err(context: &str, error: impl std::fmt::Display) -> RigaError {
        RigaError::new("gpu_error", format!("{context}: {error}"))
    }

    /// Compile `src` to a cubin (device SASS) for `arch` with NVRTC.
    ///
    /// # Safety
    /// Calls raw NVRTC FFI; the program handle is created and destroyed locally.
    unsafe fn compile_cubin(src: &str, arch: &str) -> Result<Vec<u8>, String> {
        use cudarc::nvrtc::sys as nv;
        let ok = |rc: nv::nvrtcResult| rc == nv::nvrtcResult::NVRTC_SUCCESS;
        let src_c = CString::new(src).map_err(|e| e.to_string())?;
        let name_c = CString::new("riga_kernels").expect("static name");
        let mut program: nv::nvrtcProgram = std::ptr::null_mut();
        let rc = unsafe {
            nv::nvrtcCreateProgram(
                &mut program,
                src_c.as_ptr(),
                name_c.as_ptr(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if !ok(rc) {
            return Err(format!("nvrtcCreateProgram: {rc:?}"));
        }
        let arch_c = CString::new(format!("--gpu-architecture={arch}")).expect("arch string");
        let options = [arch_c.as_ptr()];
        let rc = unsafe { nv::nvrtcCompileProgram(program, 1, options.as_ptr()) };
        if !ok(rc) {
            let mut log_size = 0usize;
            let _ = unsafe { nv::nvrtcGetProgramLogSize(program, &mut log_size) };
            let mut log = vec![0 as c_char; log_size.max(1)];
            let _ = unsafe { nv::nvrtcGetProgramLog(program, log.as_mut_ptr()) };
            let message = unsafe { CStr::from_ptr(log.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            let _ = unsafe { nv::nvrtcDestroyProgram(&mut program) };
            return Err(format!("NVRTC compile ({arch}): {message}"));
        }
        let mut size = 0usize;
        let rc = unsafe { nv::nvrtcGetCUBINSize(program, &mut size) };
        if !ok(rc) || size == 0 {
            let _ = unsafe { nv::nvrtcDestroyProgram(&mut program) };
            return Err(format!("nvrtcGetCUBINSize: {rc:?}"));
        }
        let mut cubin = vec![0u8; size];
        let rc = unsafe { nv::nvrtcGetCUBIN(program, cubin.as_mut_ptr() as *mut c_char) };
        let _ = unsafe { nv::nvrtcDestroyProgram(&mut program) };
        if !ok(rc) {
            return Err(format!("nvrtcGetCUBIN: {rc:?}"));
        }
        Ok(cubin)
    }

    /// An initialised CUDA device with the compiled kernel module.
    pub struct Gpu {
        ctx: Arc<CudaContext>,
        module: Arc<CudaModule>,
        arch: String,
    }

    impl Gpu {
        fn init() -> Result<Self, RigaError> {
            let ctx = CudaContext::new(0)
                .map_err(|e| RigaError::new("gpu_unavailable", format!("CUDA device 0: {e}")))?;
            let (major, minor) = ctx
                .compute_capability()
                .map_err(|e| gpu_err("compute capability", e))?;
            let arch = format!("sm_{major}{minor}");
            let cubin =
                unsafe { compile_cubin(KERNEL_SRC, &arch) }.map_err(|e| gpu_err("NVRTC", e))?;
            let module = ctx
                .load_module(Ptx::from_binary(cubin))
                .map_err(|e| gpu_err("module load", e))?;
            Ok(Self { ctx, module, arch })
        }

        /// The CUDA device name.
        pub fn device_name(&self) -> String {
            self.ctx
                .name()
                .unwrap_or_else(|_| "unknown-cuda-device".to_string())
        }

        /// The compute-capability target the kernels were compiled for.
        pub fn arch(&self) -> &str {
            &self.arch
        }

        /// Vectorised worst-of on the GPU. Bit-identical to the scalar CPU kernel
        /// for finite inputs.
        pub fn worst_of(&self, a: &[f32], b: &[f32]) -> Result<Vec<f32>, RigaError> {
            if a.len() != b.len() {
                return Err(RigaError::new(
                    "shape_mismatch",
                    "worst-of inputs must have equal length",
                ));
            }
            if a.is_empty() {
                return Ok(Vec::new());
            }
            let n = a.len();
            let func = self
                .module
                .load_function("worst_of")
                .map_err(|e| gpu_err("load kernel", e))?;
            let stream = self.ctx.default_stream();
            let a_dev = stream.clone_htod(a).map_err(|e| gpu_err("H2D a", e))?;
            let b_dev = stream.clone_htod(b).map_err(|e| gpu_err("H2D b", e))?;
            let mut out_dev = stream
                .alloc_zeros::<f32>(n)
                .map_err(|e| gpu_err("alloc", e))?;
            let cfg = LaunchConfig::for_num_elems(n as u32);
            let n_i32 = n as i32;
            unsafe {
                stream
                    .launch_builder(&func)
                    .arg(&a_dev)
                    .arg(&b_dev)
                    .arg(&mut out_dev)
                    .arg(&n_i32)
                    .launch(cfg)
                    .map_err(|e| gpu_err("launch", e))?;
            }
            stream.clone_dtoh(&out_dev).map_err(|e| gpu_err("D2H", e))
        }

        /// Propagate paths under a state-dependent local vol; terminal spots `[path, 2]`.
        #[allow(clippy::too_many_arguments)]
        pub fn lv_terminal_spots(
            &self,
            normals: &[f32],
            rows: &[f32],
            ls_axis: &[f32],
            s0: &[f32],
            paths: usize,
            steps: usize,
            dt: f32,
            rate: f32,
            dividends: &[f32],
        ) -> Result<Vec<f32>, RigaError> {
            let gx = ls_axis.len() / 2; // per-factor grid width; ls_axis holds 2 axes
            let func = self
                .module
                .load_function("lv_propagate")
                .map_err(|e| gpu_err("load lv kernel", e))?;
            let stream = self.ctx.default_stream();
            let normals_dev = stream
                .clone_htod(normals)
                .map_err(|e| gpu_err("H2D normals", e))?;
            let rows_dev = stream
                .clone_htod(rows)
                .map_err(|e| gpu_err("H2D rows", e))?;
            let ls_dev = stream
                .clone_htod(ls_axis)
                .map_err(|e| gpu_err("H2D ls", e))?;
            let s0_dev = stream.clone_htod(s0).map_err(|e| gpu_err("H2D s0", e))?;
            let mut out = stream
                .alloc_zeros::<f32>(paths * 2)
                .map_err(|e| gpu_err("alloc out", e))?;
            let cfg = LaunchConfig::for_num_elems(paths as u32);
            let (paths_i, steps_i, gx_i) = (paths as i32, steps as i32, gx as i32);
            let q0 = dividends[0];
            let q1 = dividends[1];
            unsafe {
                stream
                    .launch_builder(&func)
                    .arg(&normals_dev)
                    .arg(&rows_dev)
                    .arg(&ls_dev)
                    .arg(&s0_dev)
                    .arg(&mut out)
                    .arg(&paths_i)
                    .arg(&steps_i)
                    .arg(&gx_i)
                    .arg(&dt)
                    .arg(&rate)
                    .arg(&q0)
                    .arg(&q1)
                    .launch(cfg)
                    .map_err(|e| gpu_err("lv launch", e))?;
            }
            stream.clone_dtoh(&out).map_err(|e| gpu_err("D2H out", e))
        }

        /// Upload the two performance slices once and keep them resident on the
        /// device, so a caller can run the kernel repeatedly without re-transferring.
        pub fn resident_worst_of(
            &self,
            a: &[f32],
            b: &[f32],
        ) -> Result<ResidentWorstOf, RigaError> {
            if a.len() != b.len() {
                return Err(RigaError::new(
                    "shape_mismatch",
                    "worst-of inputs must have equal length",
                ));
            }
            let n = a.len();
            let func = self
                .module
                .load_function("worst_of")
                .map_err(|e| gpu_err("load kernel", e))?;
            let stream = self.ctx.default_stream();
            Ok(ResidentWorstOf {
                ctx: self.ctx.clone(),
                func,
                a: stream.clone_htod(a).map_err(|e| gpu_err("H2D a", e))?,
                b: stream.clone_htod(b).map_err(|e| gpu_err("H2D b", e))?,
                out: stream
                    .alloc_zeros::<f32>(n)
                    .map_err(|e| gpu_err("alloc", e))?,
                n: n as i32,
            })
        }
    }

    /// A resident device-side worst-of reduction (upload once, run many, download once).
    pub struct ResidentWorstOf {
        ctx: Arc<CudaContext>,
        func: cudarc::driver::CudaFunction,
        a: cudarc::driver::CudaSlice<f32>,
        b: cudarc::driver::CudaSlice<f32>,
        out: cudarc::driver::CudaSlice<f32>,
        n: i32,
    }

    impl ResidentWorstOf {
        /// Run the worst-of kernel once over the resident buffers.
        pub fn reduce(&mut self) -> Result<(), RigaError> {
            let stream = self.ctx.default_stream();
            let cfg = LaunchConfig::for_num_elems(self.n as u32);
            let n = self.n;
            unsafe {
                stream
                    .launch_builder(&self.func)
                    .arg(&self.a)
                    .arg(&self.b)
                    .arg(&mut self.out)
                    .arg(&n)
                    .launch(cfg)
                    .map_err(|e| gpu_err("launch", e))?;
            }
            Ok(())
        }

        /// Download the reduced result.
        pub fn download(&self) -> Result<Vec<f32>, RigaError> {
            self.ctx
                .default_stream()
                .clone_dtoh(&self.out)
                .map_err(|e| gpu_err("D2H", e))
        }

        /// Block until all enqueued work on the device stream has completed.
        pub fn synchronize(&self) -> Result<(), RigaError> {
            self.ctx
                .default_stream()
                .synchronize()
                .map_err(|e| gpu_err("synchronize", e))
        }
    }

    static DEVICE: OnceLock<Result<Gpu, String>> = OnceLock::new();

    fn device() -> Result<&'static Gpu, RigaError> {
        match DEVICE.get_or_init(|| Gpu::init().map_err(|e| e.message)) {
            Ok(gpu) => Ok(gpu),
            Err(message) => Err(RigaError::new("gpu_unavailable", message.clone())),
        }
    }

    /// Initialise (once) and confirm a CUDA device is usable.
    pub fn ensure_available() -> Result<(), RigaError> {
        device().map(|_| ())
    }

    /// The CUDA device name, if available.
    pub fn device_name() -> Result<String, RigaError> {
        device().map(Gpu::device_name)
    }

    /// The compiled compute-capability target, if a device is available.
    pub fn arch() -> Result<String, RigaError> {
        device().map(|gpu| gpu.arch().to_string())
    }

    /// Worst-of two performance slices on the GPU.
    pub fn worst_of(a: &[f32], b: &[f32]) -> Result<Vec<f32>, RigaError> {
        device()?.worst_of(a, b)
    }

    /// Upload two performance slices and keep them resident on the device.
    pub fn resident_worst_of(a: &[f32], b: &[f32]) -> Result<ResidentWorstOf, RigaError> {
        device()?.resident_worst_of(a, b)
    }

    /// Propagate paths under a state-dependent local vol on the GPU; terminal spots `[path, 2]`.
    #[allow(clippy::too_many_arguments)]
    pub fn lv_terminal_spots(
        normals: &[f32],
        rows: &[f32],
        ls_axis: &[f32],
        s0: &[f32],
        paths: usize,
        steps: usize,
        dt: f32,
        rate: f32,
        dividends: &[f32],
    ) -> Result<Vec<f32>, RigaError> {
        device()?.lv_terminal_spots(
            normals, rows, ls_axis, s0, paths, steps, dt, rate, dividends,
        )
    }
}

#[cfg(feature = "gpu")]
pub use imp::{
    ResidentWorstOf, arch, device_name, ensure_available, lv_terminal_spots, resident_worst_of,
    worst_of,
};

/// The GPU backend is only present in a `gpu` build.
#[cfg(not(feature = "gpu"))]
pub fn ensure_available() -> Result<(), riga_core::RigaError> {
    Err(riga_core::RigaError::new(
        "gpu_unavailable",
        "this build has the GPU backend disabled; rebuild with --features gpu",
    ))
}

/// The GPU backend is only present in a `gpu` build.
#[cfg(not(feature = "gpu"))]
pub fn device_name() -> Result<String, riga_core::RigaError> {
    Err(riga_core::RigaError::new(
        "gpu_unavailable",
        "this build has the GPU backend disabled; rebuild with --features gpu",
    ))
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;

    #[test]
    fn gpu_worst_of_is_bit_identical_to_scalar() {
        use crate::simd::worst_of_two_scalar;
        let n = 4096usize;
        let mut a = vec![0.0f32; n];
        let mut b = vec![0.0f32; n];
        let mut seed = 0x1234_5678u32;
        let mut next = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 1.8 + 0.2
        };
        for i in 0..n {
            a[i] = next();
            b[i] = next();
        }
        let gpu = worst_of(&a, &b).unwrap();
        let mut cpu = vec![0.0f32; n];
        worst_of_two_scalar(&a, &b, &mut cpu);
        for i in 0..n {
            assert_eq!(gpu[i].to_bits(), cpu[i].to_bits(), "index {i}");
        }
        println!(
            "GPU device: {} (target {})",
            device_name().unwrap(),
            arch().unwrap()
        );
    }
}

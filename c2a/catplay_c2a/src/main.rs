#![recursion_limit = "256"]

use std::{error::Error, fs, process::exit};

use catplay_c2a::{AppConfig, Main, MfiManager};
use catplay_tracing::logger::{AsyncLogger, setup_prod_logger};
use catplay_tracing::tracer::SessionTracer;
use log::{error, info, warn};

#[cfg(all(feature = "jemalloc", feature = "mimalloc"))]
compile_error!("features `jemalloc` and `mimalloc` are mutually exclusive");

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(feature = "jemalloc", not(target_arch = "riscv32")))]
#[global_allocator]
static GLOBAL_ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "jemalloc", not(target_arch = "riscv32")))]
use tikv_jemalloc_sys as _;

#[cfg(all(feature = "jemalloc", not(target_arch = "riscv32")))]
fn disable_jemalloc_thread_cache() {
    let mut enabled = false;
    let result = unsafe {
        tikv_jemalloc_sys::mallctl(
            c"thread.tcache.enabled".as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut enabled as *mut _ as *mut libc::c_void,
            std::mem::size_of_val(&enabled),
        )
    };
    if result != 0 {
        eprintln!("failed to disable jemalloc thread cache: mallctl returned {result}");
    }
}

fn run_mfi_gate(config_path: &str, count: usize) -> Result<(), Box<dyn Error>> {
    if count == 0 {
        return Err("MFi self-test count must be greater than zero".into());
    }
    let source = fs::read_to_string(config_path)?;
    let mut config: AppConfig = toml::from_str(&source)?;
    config.validate()?;
    config.mfi.selftest = false;
    let mut manager = MfiManager::new();
    manager.start(&config)?;
    let device = manager.get_device();
    for iteration in 1..=count {
        let report = device.self_test()?;
        if !report.signature_verified {
            return Err("MFi gate requires a local backend with signature verification".into());
        }
        if iteration % 100 == 0 || iteration == count {
            info!("MFi challenge gate {iteration}/{count}");
        }
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    #[cfg(all(feature = "jemalloc", not(target_arch = "riscv32")))]
    disable_jemalloc_thread_cache();

    println!("Booting!!");
    setup_prod_logger();
    AsyncLogger::set_async(true);
    let b = AsyncLogger::create_barrier();
    if std::env::var("CATPLAY_TRACING").as_deref() == Ok("1") {
        warn!("Tracing is enabled (overhead)");
        SessionTracer::enable_globally();
    }

    let mut args = std::env::args().skip(1);
    let mut config_path = std::env::var("CATPLAY_CONFIG").unwrap_or_else(|_| "/etc/catplay/catplay.conf".into());
    let mut mfi_gate = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                let Some(path) = args.next() else {
                    error!("--config requires a path");
                    drop(b);
                    exit(2);
                };
                config_path = path;
            }
            "--mfi-self-test-count" => {
                let Some(count) = args.next() else {
                    error!("--mfi-self-test-count requires an integer");
                    drop(b);
                    exit(2);
                };
                mfi_gate = match count.parse::<usize>() {
                    Ok(count) => Some(count),
                    Err(error) => {
                        error!("invalid MFi self-test count: {error}");
                        drop(b);
                        exit(2);
                    }
                };
            }
            "--help" | "-h" => {
                println!("Usage: catplay-c2a [--config PATH] [--mfi-self-test-count N]");
                drop(b);
                return;
            }
            _ => {
                error!("Unknown argument: {arg}");
                drop(b);
                exit(2);
            }
        }
    }

    if let Some(count) = mfi_gate {
        let code = match run_mfi_gate(&config_path, count) {
            Ok(()) => 0,
            Err(error) => {
                error!("MFi gate failed: {error}");
                1
            }
        };
        drop(b);
        exit(code);
    }

    info!("Starting with configuration {}", config_path);

    let mut main = Main::start(&config_path).await;
    let exit_code = match main {
        Err(err) => {
            error!("Failed to start: {}", err);
            1
        }
        Ok(ref mut main) => {
            info!("Started");

            if let Err(err) = main.do_loop().await {
                error!("Error during reconcile: {}", err);
                1
            } else {
                0
            }
        }
    };

    drop(b);
    exit(exit_code);
}

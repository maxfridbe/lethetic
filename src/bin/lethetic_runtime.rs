#[cfg(target_os = "linux")]
use lethetic::python::supervisor::{
    PackageOperation, RUNTIME_ABI, SupervisorConfig, exec_worker_process, run_package_client,
    run_supervisor, validate_runtime_image_installation,
};
#[cfg(target_os = "linux")]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
fn main() {
    let args = std::env::args_os().collect::<Vec<_>>();
    let invocation = args
        .first()
        .and_then(|path| std::path::Path::new(path).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("lethetic-runtime");

    if invocation == "lethetic-pkg" {
        let code = run_async(false, package_client(args.into_iter().skip(1).collect()));
        std::process::exit(code);
    }

    let Some(command) = args.get(1).and_then(|value| value.to_str()) else {
        fail("missing runtime command");
    };
    match command {
        "image-self-test" => {
            if args.len() != 3 || args[2] != RUNTIME_ABI {
                fail("image self-test requires the exact runtime ABI");
            }
            if let Err(error) = validate_runtime_image_installation() {
                fail(&error);
            }
        }
        "worker" => {
            if args.len() != 5 {
                fail("worker command requires UID, GID, and workspace");
            }
            let uid = parse_u32(&args[2], "worker UID");
            let gid = parse_u32(&args[3], "worker GID");
            let workspace = PathBuf::from(&args[4]);
            if let Err(error) = exec_worker_process(uid, gid, &workspace) {
                fail(&error);
            }
            unreachable!("worker exec returned without an error");
        }
        "supervisor" => {
            if args.len() != 9 {
                fail(
                    "supervisor command requires ABI, runtime ID, capability path, broker socket, workspace, UID, and GID",
                );
            }
            if args[2] != RUNTIME_ABI {
                fail("runtime ABI mismatch");
            }
            let runtime_id = parse_utf8(&args[3], "runtime ID").to_string();
            let config = SupervisorConfig {
                runtime_id,
                capability_path: PathBuf::from(&args[4]),
                broker_socket_path: PathBuf::from(&args[5]),
                workspace: PathBuf::from(&args[6]),
                worker_uid: parse_u32(&args[7], "worker UID"),
                worker_gid: parse_u32(&args[8], "worker GID"),
            };
            let code = run_async(true, async move {
                match run_supervisor(config).await {
                    Ok(code) => code,
                    Err(error) => {
                        eprintln!("lethetic runtime supervisor failed: {error}");
                        1
                    }
                }
            });
            std::process::exit(code);
        }
        _ => fail("unknown runtime command"),
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("lethetic-runtime is supported only on Linux");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn run_async<F>(multi_thread: bool, future: F) -> i32
where
    F: std::future::Future<Output = i32>,
{
    let runtime = if multi_thread {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    }
    .unwrap_or_else(|error| {
        eprintln!("could not initialize runtime: {error}");
        std::process::exit(1);
    });
    let output = runtime.block_on(future);
    runtime.shutdown_timeout(std::time::Duration::from_secs(2));
    output
}

#[cfg(target_os = "linux")]
async fn package_client(args: Vec<std::ffi::OsString>) -> i32 {
    let Some(operation) = args.first().and_then(|value| value.to_str()) else {
        eprintln!("usage: lethetic-pkg refresh | lethetic-pkg install NAME...");
        return 2;
    };
    let (operation, packages) = match operation {
        "refresh" if args.len() == 1 => (PackageOperation::Refresh, Vec::new()),
        "install" if args.len() >= 2 => {
            let mut packages = Vec::with_capacity(args.len() - 1);
            for package in &args[1..] {
                let Some(package) = package.to_str() else {
                    eprintln!("package names must be UTF-8");
                    return 2;
                };
                packages.push(package.to_string());
            }
            (PackageOperation::Install, packages)
        }
        _ => {
            eprintln!("usage: lethetic-pkg refresh | lethetic-pkg install NAME...");
            return 2;
        }
    };
    match run_package_client(operation, packages).await {
        Ok(response) => {
            if !response.output.is_empty() {
                print!("{}", response.output);
                if !response.output.ends_with('\n') {
                    println!();
                }
            }
            if response.truncated {
                eprintln!("lethetic-pkg: package-manager output was truncated");
            }
            if response.ok {
                0
            } else {
                response.exit_code.unwrap_or(1).clamp(1, 125)
            }
        }
        Err(error) => {
            eprintln!("lethetic-pkg failed: {error}");
            1
        }
    }
}

#[cfg(target_os = "linux")]
fn parse_u32(value: &std::ffi::OsStr, label: &str) -> u32 {
    parse_utf8(value, label)
        .parse()
        .unwrap_or_else(|_| fail(&format!("invalid {label}")))
}

#[cfg(target_os = "linux")]
fn parse_utf8<'a>(value: &'a std::ffi::OsStr, label: &str) -> &'a str {
    value
        .to_str()
        .unwrap_or_else(|| fail(&format!("{label} must be UTF-8")))
}

#[cfg(target_os = "linux")]
fn fail(message: &str) -> ! {
    eprintln!("lethetic-runtime: {message}");
    std::process::exit(2);
}

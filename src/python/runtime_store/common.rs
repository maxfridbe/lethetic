use chrono::{DateTime, Duration, SecondsFormat, Utc};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::Path;

pub const RUNTIME_TTL_DAYS: i64 = 14;

pub(super) fn ensure_private_directory(path: &Path, create: bool) -> Result<(), String> {
    if create {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).map_err(|error| {
            format!(
                "could not create private directory {}: {error}",
                path.display()
            )
        })?;
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "could not inspect private directory {}: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(format!("untrusted private directory {}", path.display()));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("could not canonicalize private directory: {error}"))?;
    if canonical != path {
        return Err(format!(
            "private directory is not canonical: {}",
            path.display()
        ));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|error| {
        format!(
            "could not secure private directory {}: {error}",
            path.display()
        )
    })?;
    validate_existing_private_directory(path)
}

pub(super) fn create_private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700).create(path)
}

pub(super) fn validate_owner_controlled_directory(
    path: &Path,
) -> Result<std::fs::Metadata, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "could not inspect private directory {}: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(format!("untrusted private directory {}", path.display()));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("could not canonicalize private directory: {error}"))?;
    if canonical != path {
        return Err(format!(
            "private directory is not canonical: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

pub(super) fn repair_existing_private_directory(path: &Path) -> Result<(), String> {
    validate_owner_controlled_directory(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|error| {
        format!(
            "could not secure private directory {}: {error}",
            path.display()
        )
    })?;
    validate_existing_private_directory(path)
}

pub(super) fn validate_existing_private_directory(path: &Path) -> Result<(), String> {
    let metadata = validate_owner_controlled_directory(path)?;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!("untrusted private directory {}", path.display()));
    }
    Ok(())
}

pub(super) fn ttl_deadline(timestamp: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    timestamp
        .checked_add_signed(Duration::days(RUNTIME_TTL_DAYS))
        .ok_or_else(|| "runtime TTL timestamp overflowed".to_string())
}

pub(super) fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

pub(super) fn parse_timestamp(value: &str, field: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| format!("runtime manifest {field} is invalid: {error}"))
}

pub fn generate_uuid_v4() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    fill_kernel_random(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

pub fn generate_broker_capability() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    fill_kernel_random(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn fill_kernel_random(bytes: &mut [u8]) -> Result<(), String> {
    let mut filled = 0;
    while filled < bytes.len() {
        let result = unsafe {
            libc::getrandom(bytes[filled..].as_mut_ptr().cast(), bytes.len() - filled, 0)
        };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("could not obtain kernel randomness: {error}"));
        }
        if result == 0 {
            return Err("kernel randomness returned no bytes".to_string());
        }
        filled += result as usize;
    }
    Ok(())
}

pub(super) fn validate_uuid(value: &str, label: &str) -> Result<(), String> {
    if value.len() != 36
        || !value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            }
        })
    {
        return Err(format!("{label} is not a canonical lowercase UUID"));
    }
    Ok(())
}

pub(super) fn validate_image_id(value: &str) -> Result<(), String> {
    let Some(hash) = value.strip_prefix("sha256:") else {
        return Err("runtime image ID is not an exact sha256 ID".to_string());
    };
    validate_lower_hex(hash, 64, "runtime image ID")
}

pub(super) fn validate_lower_hex(value: &str, length: usize, label: &str) -> Result<(), String> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(format!(
            "{label} must contain exactly {length} lowercase hexadecimal digits"
        ));
    }
    Ok(())
}

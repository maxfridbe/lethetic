use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn specification(name: &str) -> Result<(u64, &'static str), String> {
    // Official ggerganov/whisper.cpp Git-LFS SHA-256 objects, checked before inference.
    match name {
        "tiny" => Ok((
            77_691_713,
            "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        )),
        "base" => Ok((
            147_951_465,
            "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        )),
        _ => Err("Choose the tiny or base Whisper model".into()),
    }
}
async fn verified(path: &Path, size: u64, hash: &str) -> bool {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return false;
    };
    if file.metadata().await.map(|m| m.len()).unwrap_or(0) != size {
        return false;
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        match file.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => digest.update(&buffer[..n]),
            Err(_) => return false,
        }
    }
    format!("{:x}", digest.finalize()) == hash
}
pub(crate) async fn obtain(
    directory: &Path,
    name: &str,
    download: bool,
) -> Result<PathBuf, String> {
    let (size, hash) = specification(name)?;
    let target = directory.join(format!("ggml-{name}.bin"));
    if verified(&target, size, hash).await {
        return Ok(target);
    }
    if !download {
        return Err("Local Whisper model is not installed. Open Speech to text in the command palette to download it.".into());
    }
    tokio::fs::create_dir_all(directory)
        .await
        .map_err(|e| format!("Cannot create model cache: {e}"))?;
    let temporary = directory.join(format!("ggml-{name}.download"));
    let result = async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(600))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() < 8 && attempt.url().scheme() == "https" {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map_err(|_| "Unable to initialize model downloader")?;
        let mut response = client
            .get(format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{name}.bin"
            ))
            .send()
            .await
            .map_err(|_| "Model download failed; check your connection")?;
        if !response.status().is_success() {
            return Err(format!(
                "Model download returned HTTP {}",
                response.status().as_u16()
            ));
        }
        let mut file = tokio::fs::File::create(&temporary)
            .await
            .map_err(|e| e.to_string())?;
        let mut received = 0_u64;
        let mut digest = Sha256::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Model download interrupted")?
        {
            received += chunk.len() as u64;
            if received > size {
                return Err("Model download exceeds its expected size".into());
            }
            digest.update(&chunk);
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        if received != size || format!("{:x}", digest.finalize()) != hash {
            return Err("Model checksum verification failed; retry the download".into());
        }
        file.flush().await.map_err(|e| e.to_string())?;
        drop(file);
        // The target is exclusively managed cache data and has already failed validation.
        if target.exists() {
            tokio::fs::remove_file(&target)
                .await
                .map_err(|e| e.to_string())?;
        }
        tokio::fs::rename(&temporary, &target)
            .await
            .map_err(|e| e.to_string())?;
        Ok(target)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(temporary).await;
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cached_model_requires_size_and_hash_and_never_downloads_without_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("ggml-tiny.bin"), b"not a model")
            .await
            .unwrap();
        assert!(
            obtain(dir.path(), "tiny", false)
                .await
                .unwrap_err()
                .contains("not installed")
        );
        assert!(
            obtain(dir.path(), "../../private", true)
                .await
                .unwrap_err()
                .contains("Choose")
        );
        let path = dir.path().join("hash-test");
        tokio::fs::write(&path, b"abc").await.unwrap();
        assert!(
            verified(
                &path,
                3,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            )
            .await
        );
        assert!(!verified(&path, 3, "incorrect").await);
    }
}

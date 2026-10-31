//! The foreground media service uses the same native HTTP transport as desktop speech.
//! No loopback proxy, WebView callback, or change to Android's global HTTP policy.
use jni::{
    JNIEnv,
    objects::{JObject, JString},
    sys::jstring,
};
use lethetic_network::speech::{SpeechRequest, SpeechStreams};
use std::sync::LazyLock;

static STREAMS: LazyLock<SpeechStreams> = LazyLock::new(SpeechStreams::default);

fn input(env: &mut JNIEnv<'_>, value: &JString<'_>, limit: usize) -> Result<String, String> {
    let value: String = env
        .get_string(value)
        .map_err(|_| "Native speech argument unavailable")?
        .into();
    if value.len() > limit {
        return Err("Native speech argument too large".into());
    }
    Ok(value)
}
fn checked(
    env: &mut JNIEnv<'_>,
    action: impl FnOnce(&mut JNIEnv<'_>) -> Result<String, String>,
) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| action(env)))
        .unwrap_or_else(|_| Err("Native speech transport interrupted".into()));
    match result {
        Ok(value) => match env.new_string(value) {
            Ok(value) => value.into_raw(),
            Err(_) => {
                let _ = env.throw_new("java/io/IOException", "Could not return native speech data");
                std::ptr::null_mut()
            }
        },
        Err(message) => {
            let _ = env.throw_new("java/io/IOException", message);
            std::ptr::null_mut()
        }
    }
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_maxfridbe_lethetic_host_NativeSpeechInputStream_beginNative(
    mut env: JNIEnv<'_>,
    _object: JObject<'_>,
    id: JString<'_>,
    request: JString<'_>,
) -> jstring {
    checked(&mut env, |env| {
        let id = input(env, &id, 64)?;
        let request: SpeechRequest = serde_json::from_str(&input(env, &request, 256 * 1024)?)
            .map_err(|_| "Invalid native speech request")?;
        tauri::async_runtime::block_on(async { STREAMS.start(id, request) })?;
        Ok(String::new())
    })
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_maxfridbe_lethetic_host_NativeSpeechInputStream_nextNative(
    mut env: JNIEnv<'_>,
    _object: JObject<'_>,
    id: JString<'_>,
) -> jstring {
    checked(&mut env, |env| {
        let id = input(env, &id, 64)?;
        let event = tauri::async_runtime::block_on(STREAMS.next(&id))?;
        serde_json::to_string(&event).map_err(|_| "Could not serialize native speech event".into())
    })
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_maxfridbe_lethetic_host_NativeSpeechInputStream_cancelNative(
    mut env: JNIEnv<'_>,
    _object: JObject<'_>,
    id: JString<'_>,
) -> jstring {
    checked(&mut env, |env| {
        STREAMS.cancel(Some(&input(env, &id, 64)?))?;
        Ok(String::new())
    })
}

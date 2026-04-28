//! End-to-end smoke test for the N-API loader: builds a minimal
//! `*.node` shared library at runtime via `cc`, then `require()`s it
//! through the CommonJS pipeline and asserts that the addon's
//! exports are observable from JS.

#![allow(clippy::future_not_send)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

const ADDON_C: &str = r#"
#include <stddef.h>

typedef enum {
  napi_ok = 0,
} napi_status;

typedef struct napi_env__* napi_env;
typedef struct { void* p; } napi_value;

extern napi_status napi_create_string_utf8(napi_env, const char*, size_t, napi_value*);
extern napi_status napi_create_int32(napi_env, int, napi_value*);
extern napi_status napi_set_named_property(napi_env, napi_value, const char*, napi_value);

napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  napi_value greeting;
  napi_create_string_utf8(env, "hello", (size_t)-1, &greeting);
  napi_set_named_property(env, exports, "greeting", greeting);

  napi_value answer;
  napi_create_int32(env, 42, &answer);
  napi_set_named_property(env, exports, "answer", answer);

  return exports;
}
"#;

fn build_addon(dir: &Path) -> std::path::PathBuf {
    let src = dir.join("hello.c");
    std::fs::write(&src, ADDON_C).expect("write c source");

    let obj = dir.join("hello.o");
    let status = std::process::Command::new("cc")
        .args(["-fPIC", "-c", "-o"])
        .arg(&obj)
        .arg(&src)
        .status()
        .expect("invoke cc");
    assert!(status.success(), "cc compile failed");

    let out = dir.join("hello.node");
    let mut link = std::process::Command::new("cc");
    if cfg!(target_os = "macos") {
        link.args([
            "-dynamiclib",
            "-undefined",
            "dynamic_lookup",
            "-Wl,-flat_namespace",
        ]);
    } else {
        link.arg("-shared");
    }
    let status = link
        .arg("-o")
        .arg(&out)
        .arg(&obj)
        .status()
        .expect("invoke cc link");
    assert!(status.success(), "cc link failed");
    out
}

async fn run_module(dir: &Path, entry: &Path) -> Result<(), String> {
    let registry = Arc::new(default_registry().map_err(|e| e.to_string())?);
    let resolver = Arc::new(FsResolver::new(vec![dir.to_path_buf()], registry));
    let env = Arc::new(MapEnv::from_pairs(std::iter::empty::<(String, String)>()));
    let process = ProcessConfig::builder(env).build();
    let ctx = BootContext::new().with_cjs(resolver).with_process(process);
    V8Engine::boot_with(entry, ctx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tokio::test(flavor = "current_thread")]
async fn loads_native_addon_and_reads_exports() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addon_path = build_addon(dir.path());
    let addon_spec = addon_path.to_string_lossy().replace('\\', "\\\\");

    let entry = dir.path().join("entry.cjs");
    let body = format!(
        r#"
        const addon = require({addon_spec:?});
        if (addon.greeting !== "hello") throw new Error("greeting=" + addon.greeting);
        if (addon.answer !== 42) throw new Error("answer=" + addon.answer);
        "#
    );
    std::fs::write(&entry, body).expect("write entry");

    let dir_path = dir.path().to_path_buf();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { run_module(&dir_path, &entry).await })
        .await;
    drop(dir);
    if let Err(err) = result {
        panic!("native addon load failed: {err}");
    }
}

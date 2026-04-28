//! P3 buffers/typed-array smoke: addon allocates an ArrayBuffer, writes
//! bytes through the raw pointer N-API hands back, returns a Uint8Array
//! view, and JS verifies the content.

#![allow(clippy::future_not_send)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

const ADDON_C: &str = r#"
#include <stddef.h>
#include <stdint.h>

typedef enum { napi_ok = 0 } napi_status;
typedef struct napi_env__* napi_env;
typedef struct { void* p; } napi_value;
typedef struct { void* p; } napi_callback_info;
typedef napi_value (*napi_callback)(napi_env, napi_callback_info);

typedef enum {
  napi_int8 = 0, napi_uint8 = 1, napi_uint8_clamped = 2,
  napi_int16 = 3, napi_uint16 = 4,
  napi_int32 = 5, napi_uint32 = 6,
  napi_float32 = 7, napi_float64 = 8,
  napi_bigint64 = 9, napi_biguint64 = 10
} napi_typedarray_type;

extern napi_status napi_create_arraybuffer(napi_env, size_t, void**, napi_value*);
extern napi_status napi_get_arraybuffer_info(napi_env, napi_value, void**, size_t*);
extern napi_status napi_create_typedarray(napi_env, napi_typedarray_type, size_t,
                                          napi_value, size_t, napi_value*);
extern napi_status napi_get_typedarray_info(napi_env, napi_value,
                                            napi_typedarray_type*, size_t*,
                                            void**, napi_value*, size_t*);
extern napi_status napi_create_function(napi_env, const char*, size_t,
                                        napi_callback, void*, napi_value*);
extern napi_status napi_get_cb_info(napi_env, napi_callback_info, size_t*,
                                    napi_value*, napi_value*, void**);
extern napi_status napi_set_named_property(napi_env, napi_value, const char*, napi_value);
extern napi_status napi_create_int32(napi_env, int, napi_value*);
extern napi_status napi_get_value_int32(napi_env, napi_value, int*);
extern napi_status napi_create_buffer(napi_env, size_t, void**, napi_value*);
extern napi_status napi_get_buffer_info(napi_env, napi_value, void**, size_t*);

static napi_value make_bytes(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
  int n = 0;
  napi_get_value_int32(env, argv[0], &n);
  size_t len = (size_t)n;

  void* raw = NULL;
  napi_value ab;
  napi_create_arraybuffer(env, len, &raw, &ab);
  uint8_t* bytes = (uint8_t*)raw;
  for (size_t i = 0; i < len; i++) bytes[i] = (uint8_t)(i * 7 + 3);

  napi_value ta;
  napi_create_typedarray(env, napi_uint8, len, ab, 0, &ta);
  return ta;
}

static napi_value sum_bytes(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);

  void* data = NULL;
  size_t len = 0;
  napi_get_buffer_info(env, argv[0], &data, &len);
  uint8_t* bytes = (uint8_t*)data;
  int total = 0;
  for (size_t i = 0; i < len; i++) total += bytes[i];

  napi_value out;
  napi_create_int32(env, total, &out);
  return out;
}

static napi_value make_buffer(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  napi_get_cb_info(env, info, &argc, argv, NULL, NULL);
  int n = 0;
  napi_get_value_int32(env, argv[0], &n);
  size_t len = (size_t)n;

  void* raw = NULL;
  napi_value buf;
  napi_create_buffer(env, len, &raw, &buf);
  uint8_t* bytes = (uint8_t*)raw;
  for (size_t i = 0; i < len; i++) bytes[i] = (uint8_t)(i + 1);
  return buf;
}

napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  napi_value f1, f2, f3;
  napi_create_function(env, "makeBytes", (size_t)-1, make_bytes, NULL, &f1);
  napi_set_named_property(env, exports, "makeBytes", f1);
  napi_create_function(env, "sumBytes", (size_t)-1, sum_bytes, NULL, &f2);
  napi_set_named_property(env, exports, "sumBytes", f2);
  napi_create_function(env, "makeBuffer", (size_t)-1, make_buffer, NULL, &f3);
  napi_set_named_property(env, exports, "makeBuffer", f3);
  return exports;
}
"#;

fn build_addon(dir: &Path) -> std::path::PathBuf {
    let src = dir.join("buf.c");
    std::fs::write(&src, ADDON_C).expect("write c source");

    let obj = dir.join("buf.o");
    let status = std::process::Command::new("cc")
        .args(["-fPIC", "-c", "-o"])
        .arg(&obj)
        .arg(&src)
        .status()
        .expect("invoke cc");
    assert!(status.success(), "cc compile failed");

    let out = dir.join("buf.node");
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
async fn buffers_round_trip_through_native_addon() {
    let dir = tempfile::tempdir().expect("tempdir");
    let addon_path = build_addon(dir.path());
    let addon_spec = addon_path.to_string_lossy().replace('\\', "\\\\");

    let entry = dir.path().join("entry.cjs");
    let body = format!(
        r#"
        const addon = require({addon_spec:?});

        const ta = addon.makeBytes(8);
        if (!(ta instanceof Uint8Array)) throw new Error("not Uint8Array: " + ta);
        if (ta.length !== 8) throw new Error("bad length: " + ta.length);
        for (let i = 0; i < 8; i++) {{
          const want = (i * 7 + 3) & 0xff;
          if (ta[i] !== want) throw new Error("byte " + i + " = " + ta[i] + " want " + want);
        }}

        const total = addon.sumBytes(ta);
        let expected = 0;
        for (let i = 0; i < 8; i++) expected += (i * 7 + 3) & 0xff;
        if (total !== expected) throw new Error("sum mismatch " + total + " vs " + expected);

        const buf = addon.makeBuffer(4);
        if (!(buf instanceof Uint8Array)) throw new Error("buf not Uint8Array");
        if (buf.length !== 4) throw new Error("buf len " + buf.length);
        if (buf[0] !== 1 || buf[3] !== 4) throw new Error("buf bytes wrong");
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
        panic!("buffer round-trip failed: {err}");
    }
}

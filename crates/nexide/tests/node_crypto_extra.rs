//! Integration tests for the extended `node:crypto` surface
//! (PBKDF2, scrypt, additional ciphers, Sign/Verify).

#![allow(clippy::future_not_send, clippy::significant_drop_tightening)]

use std::path::Path;
use std::sync::Arc;

use nexide::engine::cjs::{FsResolver, default_registry};
use nexide::engine::{BootContext, V8Engine};
use nexide::ops::{MapEnv, ProcessConfig};

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

async fn assert_passes(body: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let entry = dir.path().join("entry.cjs");
    std::fs::write(&entry, body).expect("write entry");
    let dir_path = dir.path().to_path_buf();
    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(async move { run_module(&dir_path, &entry).await })
        .await;
    drop(dir);
    if let Err(err) = result {
        panic!("module failed: {err}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pbkdf2_matches_known_vector() {
    assert_passes(
        "const c = require('node:crypto');\n\
         // RFC 6070 PBKDF2-HMAC-SHA1 vector: pwd='password' salt='salt' iter=1\n\
         const out = c.pbkdf2Sync('password', 'salt', 1, 20, 'sha1');\n\
         const expected = '0c60c80f961f0e71f3a9b524af6012062fe037a6';\n\
         if (out.toString('hex') !== expected) {\n\
           throw new Error('mismatch ' + out.toString('hex'));\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn scrypt_matches_known_vector() {
    assert_passes(
        "const c = require('node:crypto');\n\
         // RFC 7914 scrypt vector: pwd='' salt='' N=16 r=1 p=1 dk=64\n\
         const out = c.scryptSync('', '', 64, { N: 16, r: 1, p: 1 });\n\
         const expected = '77d6576238657b203b19ca42c18a0497f16b4844e3074ae8dfdffa3fede21442fcd0069ded0948f8326a753a0fc81f17e8d3e0fb2e0d3628cf35e20c38d18906';\n\
         if (out.toString('hex') !== expected) {\n\
           throw new Error('scrypt mismatch ' + out.toString('hex'));\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn aes_256_cbc_round_trip() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const key = Buffer.alloc(32, 7);\n\
         const iv = Buffer.alloc(16, 3);\n\
         const enc = c.createCipheriv('aes-256-cbc', key, iv);\n\
         const ct = Buffer.concat([enc.update('hello world'), enc.final()]);\n\
         const dec = c.createDecipheriv('aes-256-cbc', key, iv);\n\
         const pt = Buffer.concat([dec.update(ct), dec.final()]);\n\
         if (pt.toString('utf8') !== 'hello world') {\n\
           throw new Error('cbc decrypt mismatch ' + pt.toString('utf8'));\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn aes_128_ctr_round_trip() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const key = c.randomBytes(16);\n\
         const iv = c.randomBytes(16);\n\
         const enc = c.createCipheriv('aes-128-ctr', key, iv);\n\
         const ct = Buffer.concat([enc.update('streaming-mode'), enc.final()]);\n\
         const dec = c.createDecipheriv('aes-128-ctr', key, iv);\n\
         const pt = Buffer.concat([dec.update(ct), dec.final()]);\n\
         if (pt.toString('utf8') !== 'streaming-mode') {\n\
           throw new Error('ctr decrypt mismatch');\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn chacha20_poly1305_round_trip() {
    assert_passes(
        "const c = require('node:crypto');\n\
         const key = c.randomBytes(32);\n\
         const nonce = c.randomBytes(12);\n\
         const enc = c.createCipheriv('chacha20-poly1305', key, nonce);\n\
         enc.setAAD(Buffer.from('hdr'));\n\
         const ct = Buffer.concat([enc.update('aead-payload'), enc.final()]);\n\
         const tag = enc.getAuthTag();\n\
         const dec = c.createDecipheriv('chacha20-poly1305', key, nonce);\n\
         dec.setAAD(Buffer.from('hdr'));\n\
         dec.setAuthTag(tag);\n\
         const pt = Buffer.concat([dec.update(ct), dec.final()]);\n\
         if (pt.toString('utf8') !== 'aead-payload') {\n\
           throw new Error('chacha decrypt mismatch');\n\
         }\n",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ed25519_sign_verify_round_trip() {
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use ed25519_dalek::pkcs8::spki::EncodePublicKey;
    use rand::RngCore;
    let mut secret = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let signing = SigningKey::from_bytes(&secret);
    let private_pem = signing
        .to_pkcs8_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
        .expect("private pem");
    let public_pem = signing
        .verifying_key()
        .to_public_key_pem(ed25519_dalek::pkcs8::spki::der::pem::LineEnding::LF)
        .expect("public pem");
    let body = format!(
        "const c = require('node:crypto');\n\
         const priv = `{}`;\n\
         const pub = `{}`;\n\
         const sig = c.createSign('ed25519').update('hello').sign(priv);\n\
         const ok = c.createVerify('ed25519').update('hello').verify(pub, sig);\n\
         if (!ok) throw new Error('ed25519 verify failed');\n\
         const bad = c.createVerify('ed25519').update('hellp').verify(pub, sig);\n\
         if (bad) throw new Error('ed25519 verify accepted wrong message');\n",
        private_pem.as_str(),
        public_pem,
    );
    assert_passes(&body).await;
}

#[tokio::test(flavor = "current_thread")]
async fn ecdsa_p256_sign_verify_round_trip() {
    use p256::ecdsa::SigningKey;
    use p256::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use rand::rngs::OsRng;
    let signing = SigningKey::random(&mut OsRng);
    let private_pem = signing.to_pkcs8_pem(LineEnding::LF).expect("private pem");
    let public_pem = signing
        .verifying_key()
        .to_public_key_pem(LineEnding::LF)
        .expect("public pem");
    let body = format!(
        "const c = require('node:crypto');\n\
         const priv = `{}`;\n\
         const pub = `{}`;\n\
         const sig = c.createSign('sha256').update('msg').sign({{ key: priv }});\n\
         const ok = c.createVerify('sha256').update('msg').verify({{ key: pub }}, sig);\n\
         if (!ok) throw new Error('ecdsa verify failed');\n",
        private_pem.as_str(),
        public_pem,
    );
    assert_passes(&body).await;
}

"use strict";

// node:crypto - surface backed by `op_crypto_*` host calls.
//
// Hash/HMAC/cipher operations are exposed via accumulating JS shells
// (the host ops are one-shot) so that `update()` chunks stay in
// memory until `digest()` / `final()` issues a single op call.
//
// Supported ciphers (createCipheriv / createDecipheriv):
//   * `aes-128-cbc`, `aes-192-cbc`, `aes-256-cbc` (PKCS#7 padded)
//   * `aes-128-ctr`, `aes-256-ctr`
//   * `aes-256-gcm` (AEAD, with `setAAD` / `getAuthTag` / `setAuthTag`)
//   * `chacha20-poly1305` (AEAD, 12-byte nonce, 16-byte tag)
//
// KDFs: `pbkdf2`, `pbkdf2Sync`, `scrypt`, `scryptSync`.
//
// Sign/Verify (`createSign` / `createVerify`):
//   * RSASSA-PKCS1-v1_5 with SHA-256 / SHA-384 / SHA-512
//   * ECDSA on the P-256 curve (DER-encoded signatures)
//   * Ed25519

const ops = Nexide.core.ops;
const Buffer = globalThis.Buffer;

function concat(chunks) {
  let total = 0; for (const c of chunks) total += c.byteLength;
  const out = new Uint8Array(total);
  let off = 0; for (const c of chunks) { out.set(c, off); off += c.byteLength; }
  return out;
}

function asBytes(input, encoding) {
  if (input instanceof Uint8Array) return input;
  if (typeof input === "string") return Buffer.from(input, encoding || "utf8");
  if (input && typeof input === "object" && input.type === "Buffer" && Array.isArray(input.data)) {
    return Buffer.from(input.data);
  }
  throw new TypeError("expected Buffer/Uint8Array/string");
}

function normalizeAlgo(algo) {
  return String(algo).toLowerCase().replace(/-/g, "").replace(/_/g, "");
}

function normalizeDigestName(name) {
  const n = normalizeAlgo(name);
  switch (n) {
    case "sha1": return "sha1";
    case "sha256": return "sha256";
    case "sha384": return "sha384";
    case "sha512": return "sha512";
    case "md5": return "md5";
    default: return n;
  }
}

class Hash {
  constructor(algo) { this._algo = normalizeDigestName(algo); this._chunks = []; this._closed = false; }
  update(data, encoding) {
    if (this._closed) throw new Error("Digest already finalized");
    this._chunks.push(asBytes(data, encoding));
    return this;
  }
  digest(encoding) {
    if (this._closed) throw new Error("Digest already finalized");
    this._closed = true;
    const out = Buffer.from(ops.op_crypto_hash(this._algo, concat(this._chunks)));
    return encoding ? out.toString(encoding) : out;
  }
  copy() {
    const c = new Hash(this._algo);
    c._chunks = this._chunks.slice();
    return c;
  }
}

class Hmac {
  constructor(algo, key) {
    this._algo = normalizeDigestName(algo);
    this._key = asBytes(key);
    this._chunks = [];
    this._closed = false;
  }
  update(data, encoding) {
    if (this._closed) throw new Error("HMAC already finalized");
    this._chunks.push(asBytes(data, encoding));
    return this;
  }
  digest(encoding) {
    if (this._closed) throw new Error("HMAC already finalized");
    this._closed = true;
    const out = Buffer.from(ops.op_crypto_hmac(this._algo, this._key, concat(this._chunks)));
    return encoding ? out.toString(encoding) : out;
  }
}

function createHash(algo) { return new Hash(algo); }
function createHmac(algo, key) { return new Hmac(algo, key); }

function randomBytes(size, cb) {
  const buf = Buffer.from(ops.op_crypto_random_bytes(size));
  if (cb) { queueMicrotask(() => cb(null, buf)); return undefined; }
  return buf;
}
function randomFillSync(buf, offset = 0, size) {
  const len = size === undefined ? buf.length - offset : size;
  const random = ops.op_crypto_random_bytes(len);
  if (buf instanceof Uint8Array) buf.set(random, offset);
  else throw new TypeError("randomFillSync requires a Uint8Array");
  return buf;
}
function randomFill(buf, offset, size, cb) {
  if (typeof offset === "function") { cb = offset; offset = 0; size = buf.length; }
  else if (typeof size === "function") { cb = size; size = buf.length - offset; }
  try {
    randomFillSync(buf, offset, size);
    queueMicrotask(() => cb(null, buf));
  } catch (err) {
    queueMicrotask(() => cb(err));
  }
}
function randomUUID() { return ops.op_crypto_random_uuid(); }
function randomInt(min, max, cb) {
  if (max === undefined) { max = min; min = 0; }
  if (typeof max === "function") { cb = max; max = min; min = 0; }
  if (!Number.isInteger(min) || !Number.isInteger(max) || max <= min) {
    const err = new RangeError("randomInt: max must be > min and both must be integers");
    if (cb) return queueMicrotask(() => cb(err));
    throw err;
  }
  const range = max - min;
  const bytes = ops.op_crypto_random_bytes(6);
  let v = 0;
  for (let i = 0; i < 6; i++) v = v * 256 + bytes[i];
  const result = min + (v % range);
  if (cb) { queueMicrotask(() => cb(null, result)); return undefined; }
  return result;
}

function timingSafeEqual(a, b) {
  return ops.op_crypto_timing_safe_equal(asBytes(a), asBytes(b));
}

function pbkdf2Sync(password, salt, iterations, keylen, digest) {
  return Buffer.from(
    ops.op_crypto_pbkdf2(asBytes(password), asBytes(salt), iterations, keylen, normalizeDigestName(digest)),
  );
}
function pbkdf2(password, salt, iterations, keylen, digest, cb) {
  queueMicrotask(() => {
    try { cb(null, pbkdf2Sync(password, salt, iterations, keylen, digest)); }
    catch (err) { cb(err); }
  });
}

function scryptSync(password, salt, keylen, options = {}) {
  const N = options.N ?? options.cost ?? 16384;
  const r = options.r ?? options.blockSize ?? 8;
  const p = options.p ?? options.parallelization ?? 1;
  return Buffer.from(
    ops.op_crypto_scrypt(asBytes(password), asBytes(salt), keylen, N, r, p),
  );
}
function scrypt(password, salt, keylen, options, cb) {
  if (typeof options === "function") { cb = options; options = {}; }
  queueMicrotask(() => {
    try { cb(null, scryptSync(password, salt, keylen, options)); }
    catch (err) { cb(err); }
  });
}

const AEAD_GCM = "aes-256-gcm";
const AEAD_CHACHA = "chacha20-poly1305";
const NON_AEAD_AES = new Set([
  "aes-128-cbc", "aes-192-cbc", "aes-256-cbc",
  "aes-128-ctr", "aes-256-ctr",
]);

class CipherIv {
  constructor(algo, key, iv, mode) {
    this._algo = String(algo).toLowerCase();
    this._mode = mode;
    this._key = asBytes(key);
    this._iv = asBytes(iv);
    this._aad = new Uint8Array(0);
    this._chunks = [];
    this._tagLength = 16;
    this._authTag = null;
    this._finalised = false;
    if (this._algo !== AEAD_GCM && this._algo !== AEAD_CHACHA && !NON_AEAD_AES.has(this._algo)) {
      const err = new Error(`Unsupported cipher "${algo}" in nexide`);
      err.code = "ERR_CRYPTO_UNKNOWN_CIPHER";
      throw err;
    }
  }
  setAAD(buf) { this._aad = asBytes(buf); return this; }
  setAuthTag(buf) {
    if (this._mode !== "decrypt") throw new Error("setAuthTag valid for decrypt only");
    this._authTag = asBytes(buf);
    return this;
  }
  setAutoPadding(_flag) { return this; }
  update(data, inputEncoding, outputEncoding) {
    this._chunks.push(asBytes(data, inputEncoding));
    if (outputEncoding) return Buffer.alloc(0).toString(outputEncoding);
    return Buffer.alloc(0);
  }
  final(encoding) {
    if (this._finalised) throw new Error("Cipher already finalized");
    this._finalised = true;
    const input = concat(this._chunks);
    let out;
    if (this._algo === AEAD_GCM) {
      if (this._mode === "encrypt") {
        const sealed = Buffer.from(ops.op_crypto_aes_gcm_seal(this._key, this._iv, input, this._aad));
        this._authTag = sealed.subarray(sealed.length - this._tagLength);
        out = sealed.subarray(0, sealed.length - this._tagLength);
      } else {
        if (!this._authTag) throw new Error("authTag required for decrypt");
        const ct = Buffer.concat([Buffer.from(input), Buffer.from(this._authTag)]);
        out = Buffer.from(ops.op_crypto_aes_gcm_open(this._key, this._iv, ct, this._aad));
      }
    } else if (this._algo === AEAD_CHACHA) {
      if (this._mode === "encrypt") {
        const sealed = Buffer.from(ops.op_crypto_chacha20_seal(this._key, this._iv, input, this._aad));
        this._authTag = sealed.subarray(sealed.length - this._tagLength);
        out = sealed.subarray(0, sealed.length - this._tagLength);
      } else {
        if (!this._authTag) throw new Error("authTag required for decrypt");
        const ct = Buffer.concat([Buffer.from(input), Buffer.from(this._authTag)]);
        out = Buffer.from(ops.op_crypto_chacha20_open(this._key, this._iv, ct, this._aad));
      }
    } else if (this._mode === "encrypt") {
      out = Buffer.from(ops.op_crypto_aes_encrypt(this._algo, this._key, this._iv, input));
    } else {
      out = Buffer.from(ops.op_crypto_aes_decrypt(this._algo, this._key, this._iv, input));
    }
    return encoding ? out.toString(encoding) : out;
  }
  getAuthTag() {
    if (!this._authTag) throw new Error("auth tag unavailable before final()");
    return Buffer.from(this._authTag);
  }
}

function createCipheriv(algo, key, iv) { return new CipherIv(algo, key, iv, "encrypt"); }
function createDecipheriv(algo, key, iv) { return new CipherIv(algo, key, iv, "decrypt"); }

const SIGN_ALGO_MAP = {
  "rsa-sha256": "rsa-sha256", "sha256withrsaencryption": "rsa-sha256",
  "rsa-sha384": "rsa-sha384", "sha384withrsaencryption": "rsa-sha384",
  "rsa-sha512": "rsa-sha512", "sha512withrsaencryption": "rsa-sha512",
  "ecdsa-with-sha256": "ecdsa-p256-sha256",
  "ed25519": "ed25519",
};

const ED25519_OID_BYTES = new Uint8Array([0x06, 0x03, 0x2b, 0x65, 0x70]);
const EC_OID_BYTES = new Uint8Array([0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]);
const RSA_OID_BYTES = new Uint8Array([
  0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01,
]);

function indexOfBytes(haystack, needle) {
  outer: for (let i = 0; i + needle.length <= haystack.length; i++) {
    for (let j = 0; j < needle.length; j++) {
      if (haystack[i + j] !== needle[j]) continue outer;
    }
    return i;
  }
  return -1;
}

function detectKeyAlgo(pem) {
  if (/BEGIN RSA (PUBLIC|PRIVATE) KEY/.test(pem)) return "rsa";
  if (/BEGIN EC (PUBLIC|PRIVATE) KEY/.test(pem)) return "ecdsa-p256";
  const match = pem.match(/-----BEGIN[^-]+-----([\s\S]*?)-----END[^-]+-----/);
  if (!match) return null;
  let body;
  try { body = Buffer.from(match[1].replace(/\s+/g, ""), "base64"); }
  catch (_err) { return null; }
  if (indexOfBytes(body, ED25519_OID_BYTES) >= 0) return "ed25519";
  if (indexOfBytes(body, EC_OID_BYTES) >= 0) return "ecdsa-p256";
  if (indexOfBytes(body, RSA_OID_BYTES) >= 0) return "rsa";
  return null;
}

function resolveSignAlgo(algo, key) {
  const lookup = String(algo).toLowerCase();
  if (SIGN_ALGO_MAP[lookup]) return SIGN_ALGO_MAP[lookup];
  if (typeof key === "object" && key && key.type === "ed25519") return "ed25519";
  const pem = (() => {
    try { return pemFromKeyLike(key); } catch (_err) { return null; }
  })();
  const keyType = pem ? detectKeyAlgo(pem) : null;
  if (lookup === "sha256" || lookup === "rsa-sha256") {
    if (keyType === "ecdsa-p256") return "ecdsa-p256-sha256";
    if (keyType === "ed25519") return "ed25519";
    return "rsa-sha256";
  }
  if (lookup === "sha384" || lookup === "rsa-sha384") return "rsa-sha384";
  if (lookup === "sha512" || lookup === "rsa-sha512") return "rsa-sha512";
  if (keyType === "ed25519") return "ed25519";
  return lookup;
}

function pemFromKeyLike(key) {
  if (typeof key === "string") return key;
  if (key && typeof key === "object") {
    if (typeof key.key === "string") return key.key;
    if (key.key instanceof Uint8Array) return new TextDecoder().decode(key.key);
  }
  if (key instanceof Uint8Array) return new TextDecoder().decode(key);
  throw new TypeError("Sign/Verify: key must be a PEM string, Buffer, or KeyObject");
}

class Sign {
  constructor(algo) { this._algo = algo; this._chunks = []; }
  update(data, encoding) { this._chunks.push(asBytes(data, encoding)); return this; }
  sign(key, encoding) {
    const algo = resolveSignAlgo(this._algo, key);
    const pem = pemFromKeyLike(key);
    const sig = Buffer.from(ops.op_crypto_sign(algo, pem, concat(this._chunks)));
    return encoding ? sig.toString(encoding) : sig;
  }
}

class Verify {
  constructor(algo) { this._algo = algo; this._chunks = []; }
  update(data, encoding) { this._chunks.push(asBytes(data, encoding)); return this; }
  verify(key, signature, signatureEncoding) {
    const algo = resolveSignAlgo(this._algo, key);
    const pem = pemFromKeyLike(key);
    const sigBytes = typeof signature === "string"
      ? Buffer.from(signature, signatureEncoding || "hex")
      : asBytes(signature);
    return ops.op_crypto_verify(algo, pem, concat(this._chunks), sigBytes);
  }
}

function createSign(algo) { return new Sign(algo); }
function createVerify(algo) { return new Verify(algo); }

const subtle = {
  async digest(algorithm, data) {
    const name = typeof algorithm === "string" ? algorithm : algorithm && algorithm.name;
    if (typeof name !== "string") throw new TypeError("digest: algorithm name required");
    const map = { "SHA-1": "sha1", "SHA-256": "sha256", "SHA-384": "sha384", "SHA-512": "sha512" };
    const algo = map[name.toUpperCase()] || name.toLowerCase().replace(/-/g, "");
    const bytes = data instanceof Uint8Array
      ? data
      : ArrayBuffer.isView(data)
        ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
        : data instanceof ArrayBuffer
          ? new Uint8Array(data)
          : null;
    if (!bytes) throw new TypeError("digest: data must be BufferSource");
    const out = ops.op_crypto_hash(algo, bytes);
    return out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength);
  },
};

const webcrypto = {
  randomUUID,
  getRandomValues(target) {
    if (!(target instanceof Uint8Array || ArrayBuffer.isView(target))) {
      throw new TypeError("getRandomValues requires a typed array");
    }
    const view = target instanceof Uint8Array
      ? target
      : new Uint8Array(target.buffer, target.byteOffset, target.byteLength);
    view.set(ops.op_crypto_random_bytes(view.length));
    return target;
  },
  subtle,
};

class CryptoKey {
  constructor() {
    const err = new TypeError("Illegal constructor");
    err.code = "ERR_INVALID_THIS";
    throw err;
  }
}

class SubtleCrypto {
  constructor() {
    const err = new TypeError("Illegal constructor");
    err.code = "ERR_INVALID_THIS";
    throw err;
  }
}

class Crypto {
  constructor() {
    const err = new TypeError("Illegal constructor");
    err.code = "ERR_INVALID_THIS";
    throw err;
  }
}

Object.setPrototypeOf(webcrypto, Crypto.prototype);
Object.setPrototypeOf(subtle, SubtleCrypto.prototype);

webcrypto.CryptoKey = CryptoKey;
webcrypto.SubtleCrypto = SubtleCrypto;
webcrypto.Crypto = Crypto;

module.exports = {
  createHash,
  createHmac,
  randomBytes,
  randomFill,
  randomFillSync,
  randomUUID,
  randomInt,
  timingSafeEqual,
  pbkdf2,
  pbkdf2Sync,
  scrypt,
  scryptSync,
  createCipheriv,
  createDecipheriv,
  createSign,
  createVerify,
  Sign,
  Verify,
  Hash,
  Hmac,
  webcrypto,
  Crypto,
  CryptoKey,
  SubtleCrypto,
  constants: {},
  getCiphers: () => [
    "aes-128-cbc", "aes-192-cbc", "aes-256-cbc",
    "aes-128-ctr", "aes-256-ctr",
    "aes-256-gcm", "chacha20-poly1305",
  ],
  getHashes: () => ["sha1", "sha256", "sha384", "sha512", "md5"],
};


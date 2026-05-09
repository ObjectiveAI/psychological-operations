//! Build-time tool. Packs a Chrome MV3 extension directory into a
//! signed CRX3 file using a committed RSA-2048 PKCS#8 private key
//! (generated on first invocation if missing).
//!
//! CRX3 wire format (per Chromium's `components/crx_file/crx3.proto`
//! and `crx_creator.cc`):
//!
//!     "Cr24"                        4 bytes magic
//!     u32_le(version=3)             4 bytes
//!     u32_le(header_size)           4 bytes
//!     header_data                   serialized CrxFileHeader protobuf
//!     zip_archive                   the extension files, zipped
//!
//! `CrxFileHeader` (we only populate two fields):
//!     field 2  (repeated AsymmetricKeyProof) sha256_with_rsa
//!     field 10000 (bytes)                    signed_header_data
//!
//! `AsymmetricKeyProof`:
//!     field 1 (bytes) public_key  — DER-encoded SubjectPublicKeyInfo
//!     field 2 (bytes) signature   — RSA PKCS#1-v1.5 + SHA-256
//!
//! `SignedData` (= signed_header_data):
//!     field 1 (bytes) crx_id      — first 16 bytes of sha256(SPKI)
//!
//! Signature input (concatenated, then SHA-256-then-PKCS#1-v1.5-signed):
//!     "CRX3 SignedData\0"          16 bytes (NUL-terminated)
//!     u32_le(signed_header_size)
//!     signed_header_data
//!     zip_archive

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use clap::Parser;
use rsa::pkcs1v15::Pkcs1v15Sign;
use rsa::pkcs8::{
    DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding,
};
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

#[derive(Parser)]
#[command(about = "Pack a Chrome MV3 extension dir into a signed CRX3.")]
struct Args {
    /// Source directory containing manifest.json and the rest of the
    /// unpacked extension.
    #[arg(long)]
    extension_dir: PathBuf,

    /// PKCS#8 PEM-encoded RSA-2048 private key. If the file doesn't
    /// exist, a new key is generated and written here (commit it so
    /// every machine derives the same extension ID).
    #[arg(long)]
    key: PathBuf,

    /// Output path for the packed .crx file.
    #[arg(long)]
    out: PathBuf,

    /// If set, write the derived extension ID (32 chars, a-p only)
    /// to this path.
    #[arg(long)]
    id_out: Option<PathBuf>,

    /// If set, write the base64-encoded SPKI public key (the value
    /// that goes into manifest.json's `key` field) to this path.
    #[arg(long)]
    pubkey_out: Option<PathBuf>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("crx-pack: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    if !args.extension_dir.is_dir() {
        return Err(format!("extension_dir does not exist: {}", args.extension_dir.display()).into());
    }

    let key = load_or_generate_key(&args.key)?;
    let pub_key = RsaPublicKey::from(&key);
    let spki_der = pub_key.to_public_key_der()?.into_vec();

    let extension_id = extension_id_from_spki(&spki_der);
    let pubkey_b64 = base64::engine::general_purpose::STANDARD.encode(&spki_der);

    println!("crx-pack: extension id = {extension_id}");
    if let Some(p) = &args.id_out {
        fs::write(p, &extension_id)?;
    }
    if let Some(p) = &args.pubkey_out {
        fs::write(p, &pubkey_b64)?;
    }

    // Build the in-memory zip of the extension directory.
    let zip_bytes = build_extension_zip(&args.extension_dir)?;

    // signed_header_data (SignedData proto, just the crx_id field).
    let crx_id_bytes: Vec<u8> = sha256(&spki_der)[..16].to_vec();
    let signed_header_data = encode_signed_data(&crx_id_bytes);

    // Sign over the canonical CRX3 message.
    let signature = sign_message(&key, &signed_header_data, &zip_bytes)?;

    // CrxFileHeader: one sha256_with_rsa entry + signed_header_data.
    let header_data = encode_crx_file_header(&spki_der, &signature, &signed_header_data);

    // Final file: magic + version + header_size + header + zip
    let mut out = fs::File::create(&args.out)?;
    out.write_all(b"Cr24")?;
    out.write_all(&3u32.to_le_bytes())?;
    out.write_all(&(header_data.len() as u32).to_le_bytes())?;
    out.write_all(&header_data)?;
    out.write_all(&zip_bytes)?;
    out.flush()?;

    println!(
        "crx-pack: wrote {} ({} bytes header + {} bytes zip)",
        args.out.display(),
        header_data.len(),
        zip_bytes.len(),
    );
    Ok(())
}

// ---------------------------------------------------------------------------

fn load_or_generate_key(path: &Path) -> Result<RsaPrivateKey, Box<dyn std::error::Error>> {
    if path.exists() {
        let pem = fs::read_to_string(path)?;
        Ok(RsaPrivateKey::from_pkcs8_pem(&pem)?)
    } else {
        eprintln!("crx-pack: generating new RSA-2048 key at {}", path.display());
        let mut rng = rand::thread_rng();
        let key = RsaPrivateKey::new(&mut rng, 2048)?;
        let pem = key.to_pkcs8_pem(LineEnding::LF)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, pem.as_bytes())?;
        Ok(key)
    }
}

fn extension_id_from_spki(spki: &[u8]) -> String {
    let hash = sha256(spki);
    let mut id = String::with_capacity(32);
    for &byte in &hash[..16] {
        id.push(char::from(b'a' + (byte >> 4)));
        id.push(char::from(b'a' + (byte & 0x0F)));
    }
    id
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn build_extension_zip(dir: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let cursor = std::io::Cursor::new(Vec::<u8>::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let mut entries: Vec<PathBuf> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    entries.sort();

    for path in entries {
        let rel = path
            .strip_prefix(dir)?
            .to_string_lossy()
            .replace('\\', "/");
        if rel == ".DS_Store" || rel.ends_with("/.DS_Store") {
            continue;
        }
        writer.start_file(rel, opts)?;
        let mut f = fs::File::open(&path)?;
        std::io::copy(&mut f, &mut writer)?;
    }

    let cursor = writer.finish()?;
    Ok(cursor.into_inner())
}

// ---------------------------------------------------------------------------
// Hand-rolled protobuf encoding (varint + length-delimited bytes).

fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(((v & 0x7F) | 0x80) as u8);
        v >>= 7;
    }
    out.push(v as u8);
}

fn write_bytes_field(out: &mut Vec<u8>, field_number: u32, data: &[u8]) {
    let tag = ((field_number as u64) << 3) | 2; // wire type 2 = length-delimited
    write_varint(out, tag);
    write_varint(out, data.len() as u64);
    out.extend_from_slice(data);
}

/// SignedData { crx_id: bytes }  — field 1.
fn encode_signed_data(crx_id: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + crx_id.len());
    write_bytes_field(&mut out, 1, crx_id);
    out
}

/// AsymmetricKeyProof { public_key: bytes, signature: bytes }
/// fields 1 and 2.
fn encode_asymmetric_key_proof(public_key: &[u8], signature: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(public_key.len() + signature.len() + 16);
    write_bytes_field(&mut out, 1, public_key);
    write_bytes_field(&mut out, 2, signature);
    out
}

/// CrxFileHeader: sha256_with_rsa (field 2, repeated) +
/// signed_header_data (field 10000, bytes).
fn encode_crx_file_header(public_key: &[u8], signature: &[u8], signed_header: &[u8]) -> Vec<u8> {
    let proof = encode_asymmetric_key_proof(public_key, signature);
    let mut out = Vec::with_capacity(proof.len() + signed_header.len() + 16);
    write_bytes_field(&mut out, 2, &proof);
    write_bytes_field(&mut out, 10000, signed_header);
    out
}

// ---------------------------------------------------------------------------

fn sign_message(
    key: &RsaPrivateKey,
    signed_header_data: &[u8],
    zip_bytes: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Per Chromium: SHA256 of the canonical message.
    let mut hasher = Sha256::new();
    hasher.update(b"CRX3 SignedData\0"); // 16 bytes including the NUL
    hasher.update(&(signed_header_data.len() as u32).to_le_bytes());
    hasher.update(signed_header_data);
    hasher.update(zip_bytes);
    let digest = hasher.finalize();

    let sig = key.sign(Pkcs1v15Sign::new::<Sha256>(), &digest)?;
    Ok(sig)
}


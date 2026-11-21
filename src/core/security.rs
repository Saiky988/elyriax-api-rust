use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::{distributions::Alphanumeric, Rng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtUserClaims {
    pub id: i64,
    pub username: String,
    pub email: Option<String>,
    pub avatar: Option<String>,
    #[serde(default)]
    pub providers: Vec<String>,
    pub exp: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrTokenClaims {
    pub amount: f64,
    pub content: String,
    pub exp: usize,
}

/// Derive 32-byte AES key via SHA-256(secret), 100% compatible with legacy Node.js & Python implementation.
fn derive_aes_key(secret: &str) -> AppResult<[u8; 32]> {
    if secret.is_empty() {
        return Err(AppError::internal(
            "FATAL: GENSHIN_COOKIE_SECRET is missing in environment",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    let digest = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    Ok(key)
}

/// Encrypt text using AES-256-CBC with PKCS7 padding, output format: `{iv_hex}:{ciphertext_hex}`.
pub fn encrypt_cookie(plaintext: &str, secret: &str) -> AppResult<String> {
    let key = derive_aes_key(secret)?;
    let mut iv = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut iv);

    let ciphertext = Aes256CbcEnc::new_from_slices(&key, &iv)
        .map_err(|_| AppError::internal("Failed to initialize AES-256-CBC encryptor"))?
        .encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes());

    Ok(format!("{}:{}", hex::encode(iv), hex::encode(ciphertext)))
}

/// Decrypt text encrypted in `{iv_hex}:{ciphertext_hex}` format (100% compatible with existing DB records).
pub fn decrypt_cookie(encrypted: &str, secret: &str) -> AppResult<String> {
    let (iv_hex, cipher_hex) = encrypted
        .split_once(':')
        .ok_or_else(|| AppError::bad_request("Dữ liệu Cookie đã bị hỏng hoặc không đúng định dạng."))?;

    let iv = hex::decode(iv_hex)
        .map_err(|_| AppError::bad_request("Dữ liệu Cookie đã bị hỏng hoặc Secret Key không đúng."))?;
    let ciphertext = hex::decode(cipher_hex)
        .map_err(|_| AppError::bad_request("Dữ liệu Cookie đã bị hỏng hoặc Secret Key không đúng."))?;
    let key = derive_aes_key(secret)?;

    let plaintext = Aes256CbcDec::new_from_slices(&key, &iv)
        .map_err(|_| AppError::bad_request("Dữ liệu Cookie đã bị hỏng hoặc Secret Key không đúng."))?
        .decrypt_padded_vec_mut::<Pkcs7>(&ciphertext)
        .map_err(|_| AppError::bad_request("Dữ liệu Cookie đã bị hỏng hoặc Secret Key không đúng."))?;

    String::from_utf8(plaintext)
        .map_err(|_| AppError::bad_request("Dữ liệu Cookie giải mã không phải UTF-8 hợp lệ."))
}

pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn create_jwt_token(
    id: i64,
    username: String,
    email: Option<String>,
    avatar: Option<String>,
    providers: Vec<String>,
    secret: &str,
    expires_days: i64,
) -> AppResult<String> {
    let exp = (Utc::now() + Duration::days(expires_days)).timestamp() as usize;
    let claims = JwtUserClaims {
        id,
        username,
        email,
        avatar,
        providers,
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AppError::internal(format!("Failed to sign JWT: {}", e)))
}

pub fn decode_jwt_token(token: &str, secret: &str) -> AppResult<JwtUserClaims> {
    let data = decode::<JwtUserClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::unauthorized("Token không hợp lệ"))?;
    Ok(data.claims)
}

pub fn create_qr_token(amount: f64, content: String, secret: &str) -> AppResult<String> {
    let exp = (Utc::now() + Duration::hours(2)).timestamp() as usize;
    let claims = QrTokenClaims {
        amount,
        content,
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AppError::internal(format!("Failed to sign QR token: {}", e)))
}

pub fn decode_qr_token(token: &str, secret: &str) -> AppResult<QrTokenClaims> {
    let data = decode::<QrTokenClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::forbidden("Token invalid or expired"))?;
    Ok(data.claims)
}

pub fn generate_transaction_code() -> String {
    let mut bytes = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut bytes);
    let b64 = BASE64.encode(bytes);
    let cleaned: String = b64
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(6)
        .collect::<String>()
        .to_uppercase();
    format!("ELYRIAX {}", cleaned)
}

pub fn generate_order_code() -> String {
    let mut bytes = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut bytes);
    let hex_str = hex::encode_upper(bytes);
    format!("ELYRIAX-ORD-{}", &hex_str[..6])
}

pub fn generate_hex_token(num_bytes: usize) -> String {
    let mut bytes = vec![0u8; num_bytes];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn generate_short_id(len: usize, lowercase_only: bool) -> String {
    if lowercase_only {
        const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut rng = rand::thread_rng();
        (0..len)
            .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
            .collect()
    } else {
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(len)
            .map(char::from)
            .collect()
    }
}

pub fn generate_slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_dash = true;
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

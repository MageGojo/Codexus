//! PKCE（Proof Key for Code Exchange）生成，与 Codex CLI 一致（S256）。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct PkceCodes {
    pub code_verifier: String,
    pub code_challenge: String,
}

/// 生成一对 PKCE 码：`code_challenge = base64url(sha256(code_verifier))`。
pub fn generate_pkce() -> PkceCodes {
    // CLIProxyAPI 的 Codex 登录实现使用 96 字节随机数，
    // base64url 后得到 128 字符 verifier。
    let code_verifier = random_urlsafe(96);
    let digest = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(digest);
    PkceCodes {
        code_verifier,
        code_challenge,
    }
}

/// 生成 `n` 字节随机数据的 base64url（无填充）编码，用于 verifier / state。
pub fn random_urlsafe(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_matches_verifier() {
        let pkce = generate_pkce();
        // 重新计算 challenge 应一致
        let expect = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.code_verifier.as_bytes()));
        assert_eq!(pkce.code_challenge, expect);
        // 无填充、URL 安全
        assert!(!pkce.code_challenge.contains('='));
        assert!(!pkce.code_challenge.contains('+'));
        assert!(!pkce.code_challenge.contains('/'));
    }

    #[test]
    fn randomness_differs() {
        assert_ne!(random_urlsafe(32), random_urlsafe(32));
    }
}

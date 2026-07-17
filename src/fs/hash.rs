//! 文件哈希计算（真实读取文件流式计算）

use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// 计算指定算法的哈希值，返回十六进制小写字符串
pub fn compute(path: &Path, algo: &str) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut buf = [0u8; 65536];

    macro_rules! run {
        ($hasher:expr) => {{
            let mut h = $hasher;
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                h.update(&buf[..n]);
            }
            Ok(hex(&h.finalize()))
        }};
    }

    match algo {
        "MD5" => run!(Md5::new()),
        "SHA-1" => run!(Sha1::new()),
        "SHA-256" => run!(Sha256::new()),
        "SHA-512" => run!(Sha512::new()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "不支持的哈希算法",
        )),
    }
}

/// 根据期望哈希字符串的长度自动识别算法，计算文件实际哈希并比对。
/// 返回 (识别出的算法, 是否匹配)。
pub fn verify(path: &Path, expected: &str) -> io::Result<(String, bool)> {
    let exp = expected.trim().to_lowercase();
    if exp.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "请输入要校验的哈希值",
        ));
    }
    if !exp.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "哈希值只能包含十六进制字符",
        ));
    }
    // 按十六进制字符长度推断算法
    let algo = match exp.len() {
        32 => "MD5",
        40 => "SHA-1",
        64 => "SHA-256",
        128 => "SHA-512",
        n => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "无法识别的哈希长度（{} 位），仅支持 MD5/SHA-1/SHA-256/SHA-512",
                    n
                ),
            ))
        }
    };
    let actual = compute(path, algo)?;
    Ok((algo.to_string(), actual == exp))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_known_hashes() {
        // "abc" 的标准哈希值
        let mut p = std::env::temp_dir();
        p.push(format!("ferrox_hash_{}.txt", std::process::id()));
        fs::write(&p, b"abc").unwrap();

        assert_eq!(
            compute(&p, "MD5").unwrap(),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(
            compute(&p, "SHA-1").unwrap(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            compute(&p, "SHA-256").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_file(&p).ok();
    }

    #[test]
    fn test_verify() {
        let mut p = std::env::temp_dir();
        p.push(format!("ferrox_verify_{}.txt", std::process::id()));
        fs::write(&p, b"abc").unwrap();

        // 大小写与首尾空白不敏感
        let (algo, ok) = verify(&p, "  900150983CD24FB0D6963F7D28E17F72 ").unwrap();
        assert_eq!(algo, "MD5");
        assert!(ok);

        // SHA-256 不匹配
        let (algo, ok) = verify(
            &p,
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap();
        assert_eq!(algo, "SHA-256");
        assert!(!ok);

        // 非法长度报错
        assert!(verify(&p, "abcd").is_err());
        // 非十六进制报错
        assert!(verify(&p, "zzz0150983cd24fb0d6963f7d28e17f72").is_err());

        fs::remove_file(&p).ok();
    }
}

//! ssh-keys
//!
//! this library provides pure-rust parsing, manipulation, and some basic
//! validation of ssh keys. it provides a struct for encapsulation of ssh keys
//! in projects.
//!
//! ssh-keys doesn't have the ability to generate ssh-keys. however, it does
//! allow you to construct rsa and dsa keys from their components, so if you
//! generate the keys with another library (say, rust-openssl), then you can
//! output the ssh public keys with this library.
#![allow(unused_doc_comment)]

extern crate base64;
extern crate byteorder;
extern crate crypto;
#[macro_use]
extern crate error_chain;

mod reader;
mod writer;

pub mod errors {
    error_chain! {
        foreign_links {
            Utf8(::std::str::Utf8Error);
        }
        errors {
            InvalidFormat {
                description("invalid key format")
                    display("invalid key format")
            }
            UnsupportedKeytype(t: String) {
                description("unsupported keytype")
                    display("unsupported keytype: {}", t)
            }
            UnsupportedCurve(t: String) {
                description("unsupported curve")
                    display("unsupported curve: {}", t)
            }
        }
    }
}

use errors::*;

use crypto::digest::Digest;
use crypto::sha2::Sha256;

use reader::Reader;
use writer::Writer;

use std::fmt;

const SSH_RSA: &'static str = "ssh-rsa";
const SSH_DSA: &'static str = "ssh-dss";
const SSH_ED25519: &'static str = "ssh-ed25519";
const SSH_ECDSA_256: &'static str = "ecdsa-sha2-nistp256";
const SSH_ECDSA_384: &'static str = "ecdsa-sha2-nistp384";
const SSH_ECDSA_521: &'static str = "ecdsa-sha2-nistp521";
const NISTP_256: &'static str = "nistp256";
const NISTP_384: &'static str = "nistp384";
const NISTP_521: &'static str = "nistp521";

/// Curves for ECDSA
#[derive(Clone, Debug)]
pub enum Curve {
    Nistp256,
    Nistp384,
    Nistp521,
}


impl Curve {
    /// get converts a curve name of the type in the format described in
    /// https://tools.ietf.org/html/rfc5656#section-10 and returns a curve
    /// object.
    fn get(curve: &str) -> Result<Self> {
        Ok(match curve {
            NISTP_256 => Curve::Nistp256,
            NISTP_384 => Curve::Nistp384,
            NISTP_521 => Curve::Nistp521,
            _ => return Err(ErrorKind::UnsupportedCurve(curve.to_string()).into())
        })
    }

    /// curvetype gets the curve name in the format described in
    /// https://tools.ietf.org/html/rfc5656#section-10
    fn curvetype(&self) -> &'static str {
        match *self {
            Curve::Nistp256 => NISTP_256,
            Curve::Nistp384 => NISTP_384,
            Curve::Nistp521 => NISTP_521,
        }
    }
}

impl fmt::Display for Curve {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.curvetype())
    }
}

/// Data is the representation of the data section of an ssh public key. it is
/// an enum with all the different supported key algorithms.
#[derive(Clone, Debug)]
pub enum Data {
    Rsa {
        exponent: Vec<u8>,
        modulus: Vec<u8>,
    },
    Dsa {
        p: Vec<u8>,
        q: Vec<u8>,
        g: Vec<u8>,
        pub_key: Vec<u8>,
    },
    Ed25519 {
        key: Vec<u8>,
    },
    Ecdsa {
        curve: Curve,
        key: Vec<u8>,
    },
}

/// PublicKey is the struct representation of an ssh public key.
#[derive(Clone, Debug)]
pub struct PublicKey {
    data: Data,
    comment: Option<String>,
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_key_file())
    }
}

impl PublicKey {
    /// parse takes a string and reads from it an ssh public key
    /// it uses the first part of the key to determine the keytype
    /// the format it expects is described here https://tools.ietf.org/html/rfc4253#section-6.6
    ///
    /// You can parse and output ssh keys like this
    /// ```
    /// let rsa_key = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCcMCOEryBa8IkxXacjIawaQPp08hR5h7+4vZePZ7DByTG3tqKgZYRJ86BaR+4fmdikFoQjvLJVUmwniq3wixhkP7VLCbqip3YHzxXrzxkbPC3w3O1Bdmifwn9cb8RcZXfXncCsSu+h5XCtQ5BOi41Iit3d13gIe/rfXVDURmRanV6R7Voljxdjmp/zyReuzc2/w5SI6Boi4tmcUlxAI7sFuP1kA3pABDhPtc3TDgAcPUIBoDCoY8q2egI197UuvbgsW2qraUcuQxbMvJOMSFg2FQrE2bpEqC4CtBn7+HiJrkVOHjV7bvSv7jd1SuX5XqkwMCRtdMuRpJr7CyZoFL5n demos@anduin";
    /// let key = PublicKey::parse(rsa_key).unwrap();
    /// let out = key.to_string();
    /// assert_eq!(rsa_key, out);
    /// ```
    ///
    /// parse somewhat attempts to keep track of comments, but it doesn't fully
    /// comply with the rfc in that regard.
    pub fn parse(key: &str) -> Result<Self> {
        let mut parts = key.split_whitespace();
        let keytype = parts.next().ok_or(ErrorKind::InvalidFormat)?;
        let data = parts.next().ok_or(ErrorKind::InvalidFormat)?;
        // comment is not required. if we get an empty comment (because of a
        // trailing space) throw it out.
        let comment = parts.next().and_then(|c| if c.is_empty() { None } else { Some(c.to_string()) });

        let buf = base64::decode(data)
            .chain_err(|| ErrorKind::InvalidFormat)?;
        let mut reader = Reader::new(&buf);
        let data_keytype = reader.read_string()?;
        if keytype != data_keytype {
            return Err(ErrorKind::InvalidFormat.into());
        }

        let data = match keytype {
            SSH_RSA => {
                // the data for an rsa key consists of three pieces:
                //    ssh-rsa public-exponent modulus
                // see ssh-rsa format in https://tools.ietf.org/html/rfc4253#section-6.6
                let e = reader.read_mpint()?;
                let n = reader.read_mpint()?;
                Data::Rsa {
                    exponent: e.into(),
                    modulus: n.into(),
                }
            },
            SSH_DSA => {
                // the data stored for a dsa key is, in order
                //    ssh-dsa p q g public-key
                // p and q are primes
                // g = h^((p-1)/q) where 1 < h < p-1
                // public-key is the value that is actually generated in
                // relation to the secret key
                // see https://en.wikipedia.org/wiki/Digital_Signature_Algorithm
                // and ssh-dss format in https://tools.ietf.org/html/rfc4253#section-6.6
                // and https://github.com/openssh/openssh-portable/blob/master/sshkey.c#L743
                let p = reader.read_mpint()?;
                let q = reader.read_mpint()?;
                let g = reader.read_mpint()?;
                let pub_key = reader.read_mpint()?;
                Data::Dsa {
                    p: p.into(),
                    q: q.into(),
                    g: g.into(),
                    pub_key: pub_key.into(),
                }
            },
            SSH_ED25519 => {
                // the data stored for an ed25519 is just the point on the curve
                // for now the exact specification of the point on that curve is
                // a mystery to me, instead of having to compute it, we just
                // assume the key we got is correct and copy that verbatim. this
                // also means we have to disallow arbitrary construction until
                // furthur notice.
                // see https://github.com/openssh/openssh-portable/blob/master/sshkey.c#L772
                let key = reader.read_bytes()?;
                Data::Ed25519 {
                    key: key.into(),
                }
            },
            SSH_ECDSA_256 | SSH_ECDSA_384 | SSH_ECDSA_521 => {
                // ecdsa is of the form
                //    ecdsa-sha2-[identifier] [identifier] [data]
                // the identifier is one of nistp256, nistp384, nistp521
                // the data is some weird thing described in section 2.3.4 and
                // 2.3.4 of https://www.secg.org/sec1-v2.pdf so for now we
                // aren't going to bother actually computing it and instead we
                // will just not let you construct them.
                //
                // see the data definition at
                // https://tools.ietf.org/html/rfc5656#section-3.1
                // and the openssh output
                // https://github.com/openssh/openssh-portable/blob/master/sshkey.c#L753
                // and the openssh buffer writer implementation
                // https://github.com/openssh/openssh-portable/blob/master/sshbuf-getput-crypto.c#L192
                // and the openssl point2oct implementation
                // https://github.com/openssl/openssl/blob/aa8f3d76fcf1502586435631be16faa1bef3cdf7/crypto/ec/ec_oct.c#L82
                let curve = reader.read_string()?;
                let key = reader.read_bytes()?;
                Data::Ecdsa {
                    curve: Curve::get(curve)?,
                    key: key.into(),
                }
            },
            _ => return Err(ErrorKind::UnsupportedKeytype(keytype.into()).into()),
        };

        Ok(PublicKey {
            data: data,
            comment: comment,
        })
    }

    /// get an ssh public key from rsa components
    pub fn from_rsa(e: Vec<u8>, n: Vec<u8>) -> Self {
        PublicKey {
            data: Data::Rsa {
                exponent: e,
                modulus: n,
            },
            comment: None,
        }
    }

    /// get an ssh public key from dsa components
    pub fn from_dsa(p: Vec<u8>, q: Vec<u8>, g: Vec<u8>, pkey: Vec<u8>) -> Self {
        PublicKey {
            data: Data::Dsa {
                p: p,
                q: q,
                g: g,
                pub_key: pkey,
            },
            comment: None,
        }
    }

    /// keytype returns the type of key in the format described by rfc4253
    /// The output will be ssh-{type} where type is [rsa,ed25519,ecdsa,dsa]
    pub fn keytype(&self) -> &'static str {
        match self.data {
            Data::Rsa{..} => SSH_RSA,
            Data::Dsa{..} => SSH_DSA,
            Data::Ed25519{..} => SSH_ED25519,
            Data::Ecdsa{ref curve,..} => match *curve {
                Curve::Nistp256 => SSH_ECDSA_256,
                Curve::Nistp384 => SSH_ECDSA_384,
                Curve::Nistp521 => SSH_ECDSA_521,
            },
        }
    }

    /// data returns the data section of the key in the format described by rfc4253
    /// the contents of the data section depend on the keytype. For RSA keys it
    /// contains the keytype, exponent, and modulus in that order. Other types
    /// have other data sections. This function doesn't base64 encode the data,
    /// that task is left to the consumer of the output.
    pub fn data(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_string(self.keytype());
        match self.data {
            Data::Rsa{ref exponent, ref modulus} => {
                // the data for an rsa key consists of three pieces:
                //    ssh-rsa public-exponent modulus
                // see ssh-rsa format in https://tools.ietf.org/html/rfc4253#section-6.6
                writer.write_mpint(exponent.clone());
                writer.write_mpint(modulus.clone());
            }
            Data::Dsa{ref p, ref q, ref g, ref pub_key} => {
                writer.write_mpint(p.clone());
                writer.write_mpint(q.clone());
                writer.write_mpint(g.clone());
                writer.write_mpint(pub_key.clone());
            }
            Data::Ed25519{ref key} => {
                writer.write_bytes(key.clone());
            }
            Data::Ecdsa{ref curve, ref key} => {
                writer.write_string(curve.curvetype());
                writer.write_bytes(key.clone());
            }
        }
        writer.to_vec()
    }

    pub fn set_comment(&mut self, comment: &str) {
        self.comment = Some(comment.to_string());
    }

    /// to_string returns a string representation of the ssh key
    /// this string output is appropriate to use as a public key file
    /// it adheres to the format described in https://tools.ietf.org/html/rfc4253#section-6.6
    /// an ssh key consists of three pieces:
    ///    ssh-keytype data comment
    /// each of those is encoded as big-endian bytes preceeded by four bytes
    /// representing their length.
    pub fn to_key_file(&self) -> String {
        format!("{} {} {}", self.keytype(), base64::encode(&self.data()), self.comment.clone().unwrap_or_default())
    }

    /// size returns the size of the stored ssh key
    /// for rsa keys this is determined by the number of bits in the modulus
    /// for dsa keys it's the number of bits in the prime p
    /// see https://github.com/openssh/openssh-portable/blob/master/sshkey.c#L261
    pub fn size(&self) -> usize {
        match self.data {
            Data::Rsa{ref modulus,..} => modulus.len()*8,
            Data::Dsa{ref p,..} => p.len()*8,
            Data::Ed25519{..} => 256, // ??
            Data::Ecdsa{ref curve,..} => match *curve {
                Curve::Nistp256 => 256,
                Curve::Nistp384 => 384,
                Curve::Nistp521 => 521,
            }
        }
    }

    /// fingerprint returns a string representing the fingerprint of the ssh key
    /// the format of the fingerprint is described tersely in
    /// https://tools.ietf.org/html/rfc4716#page-6. This uses the ssh-keygen
    /// defaults of a base64 encoded SHA256 hash.
    pub fn fingerprint(&self) -> String {
        let data = self.data();
        let mut hasher = Sha256::new();
        hasher.input(&data);
        let mut hashed: [u8; 32] = [0; 32];
        hasher.result(&mut hashed);
        let mut fingerprint = base64::encode(&hashed);
        // trim padding characters off the end. I'm not clear on exactly what
        // this is doing but they do it here and the test fails without it
        // https://github.com/openssh/openssh-portable/blob/643c2ad82910691b2240551ea8b14472f60b5078/sshkey.c#L918
        match fingerprint.find('=') {
            Some(l) => { fingerprint.split_off(l); },
            None => {},
        }
        format!("SHA256:{}", fingerprint)
    }

    /// to_fingerprint_string prints out the fingerprint in the same format used
    /// by `ssh-keygen -l -f key`, specifically the implementation here -
    /// https://github.com/openssh/openssh-portable/blob/master/ssh-keygen.c#L842
    /// right now it just sticks with the defaults of a base64 encoded SHA256
    /// hash.
    pub fn to_fingerprint_string(&self) -> String {
        let keytype = match self.data {
            Data::Rsa{..} => "RSA",
            Data::Dsa{..} => "DSA",
            Data::Ed25519{..} => "ED25519",
            Data::Ecdsa{..} => "ECDSA",
        };

        format!("{} {} {} ({})", self.size(), self.fingerprint(), self.comment.clone().unwrap_or("no comment".to_string()), keytype)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_RSA_KEY: &'static str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCYH3vPUJThzriVlVKmKOg71EOVYm274oRa5KLWEoK0HmjMc9ru0j4ofouoeW/AVmRVujxfaIGR/8en/lUPkiv5DSeM6aXnDz5cExNptrAy/sMPLQhVALRrqQ+dkS9Ct/YA+A1Le5LPh4MJu79hCDLTwqSdKqDuUcYQzR0M7APslaDCR96zY+VUL4lKObUUd4wsP3opdTQ6G20qXEer14EPGr9N53S/u+JJGLoPlb1uPIH96oKY4t/SeLIRQsocdViRaiF/Aq7kPzWd/yCLVdXJSRt3CftboV4kLBHGteTS551J32MJoqjEi4Q/DucWYrQfx5H3qXVB+/G2HurKPIHL demos@siril";
    const TEST_RSA_COMMENT_KEY: &'static str = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCYH3vPUJThzriVlVKmKOg71EOVYm274oRa5KLWEoK0HmjMc9ru0j4ofouoeW/AVmRVujxfaIGR/8en/lUPkiv5DSeM6aXnDz5cExNptrAy/sMPLQhVALRrqQ+dkS9Ct/YA+A1Le5LPh4MJu79hCDLTwqSdKqDuUcYQzR0M7APslaDCR96zY+VUL4lKObUUd4wsP3opdTQ6G20qXEer14EPGr9N53S/u+JJGLoPlb1uPIH96oKY4t/SeLIRQsocdViRaiF/Aq7kPzWd/yCLVdXJSRt3CftboV4kLBHGteTS551J32MJoqjEi4Q/DucWYrQfx5H3qXVB+/G2HurKPIHL test";
    const TEST_DSA_KEY: &'static str = "ssh-dss AAAAB3NzaC1kc3MAAACBAIkd9CkqldM2St8f53rfJT7kPgiA8leZaN7hdZd48hYJyKzVLoPdBMaGFuOwGjv0Im3JWqWAewANe0xeLceQL0rSFbM/mZV+1gc1nm1WmtVw4KJIlLXl3gS7NYfQ9Ith4wFnZd/xhRz9Q+MBsA1DgXew1zz4dLYI46KmFivJ7XDzAAAAFQC8z4VIhI4HlHTvB7FdwAfqWsvcOwAAAIBEqPIkW3HHDTSEhUhhV2AlIPNwI/bqaCXy2zYQ6iTT3oUh+N4xlRaBSvW+h2NC97U8cxd7Y0dXIbQKPzwNzRX1KA1F9WAuNzrx9KkpCg2TpqXShhp+Sseb+l6uJjthIYM6/0dvr9cBDMeExabPPgBo3Eii2NLbFSqIe86qav8hZAAAAIBk5AetZrG8varnzv1khkKh6Xq/nX9r1UgIOCQos2XOi2ErjlB9swYCzReo1RT7dalITVi7K9BtvJxbutQEOvN7JjJnPJs+M3OqRMMF+anXPdCWUIBxZUwctbkAD5joEjGDrNXHQEw9XixZ9p3wudbISnPFgZhS1sbS9Rlw5QogKg== demos@siril";
    const TEST_ED25519_KEY: &'static str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAhBr6++FQXB8kkgOMbdxBuyrHzuX5HkElswrN6DQoN/ demos@siril";
    const TEST_ECDSA256_KEY: &'static str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBIhfLQrww4DlhYzbSWXoX3ctOQ0jVosvfHfW+QWVotksbPzM2YgkIikTpoHUfZrYpJKWx7WYs5aqeLkdCDdk+jk= demos@siril";

    #[test]
    fn rsa_parse_to_string() {
        let key = PublicKey::parse(TEST_RSA_KEY).unwrap();
        let out = key.to_string();
        assert_eq!(TEST_RSA_KEY, out);
    }

    #[test]
    fn rsa_size() {
        let key = PublicKey::parse(TEST_RSA_KEY).unwrap();
        assert_eq!(2048, key.size());
    }

    #[test]
    fn rsa_keytype() {
        let key = PublicKey::parse(TEST_RSA_KEY).unwrap();
        assert_eq!("ssh-rsa", key.keytype());
    }

    #[test]
    fn rsa_fingerprint() {
        let key = PublicKey::parse(TEST_RSA_KEY).unwrap();
        assert_eq!("SHA256:YTw/JyJmeAAle1/7zuZkPP0C73BQ+6XrFEt2/Wy++2o", key.fingerprint());
    }

    #[test]
    fn rsa_fingerprint_string() {
        let key = PublicKey::parse(TEST_RSA_KEY).unwrap();
        assert_eq!("2048 SHA256:YTw/JyJmeAAle1/7zuZkPP0C73BQ+6XrFEt2/Wy++2o demos@siril (RSA)", key.to_fingerprint_string());
    }

    #[test]
    fn rsa_set_comment() {
        let mut key = PublicKey::parse(TEST_RSA_KEY).unwrap();
        key.set_comment("test");
        let out = key.to_string();
        assert_eq!(TEST_RSA_COMMENT_KEY, out);
    }

    #[test]
    fn dsa_parse_to_string() {
        let key = PublicKey::parse(TEST_DSA_KEY).unwrap();
        let out = key.to_string();
        assert_eq!(TEST_DSA_KEY, out);
    }

    #[test]
    fn dsa_size() {
        let key = PublicKey::parse(TEST_DSA_KEY).unwrap();
        assert_eq!(1024, key.size());
    }

    #[test]
    fn dsa_keytype() {
        let key = PublicKey::parse(TEST_DSA_KEY).unwrap();
        assert_eq!("ssh-dss", key.keytype());
    }

    #[test]
    fn dsa_fingerprint() {
        let key = PublicKey::parse(TEST_DSA_KEY).unwrap();
        assert_eq!("SHA256:/Pyxrjot1Hs5PN2Dpg/4pK2wxxtP9Igc3sDTAWIEXT4", key.fingerprint());
    }

    #[test]
    fn dsa_fingerprint_string() {
        let key = PublicKey::parse(TEST_DSA_KEY).unwrap();
        assert_eq!("1024 SHA256:/Pyxrjot1Hs5PN2Dpg/4pK2wxxtP9Igc3sDTAWIEXT4 demos@siril (DSA)", key.to_fingerprint_string());
    }

    #[test]
    fn ed25519_parse_to_string() {
        let key = PublicKey::parse(TEST_ED25519_KEY).unwrap();
        let out = key.to_string();
        assert_eq!(TEST_ED25519_KEY, out);
    }

    #[test]
    fn ed25519_size() {
        let key = PublicKey::parse(TEST_ED25519_KEY).unwrap();
        assert_eq!(256, key.size());
    }

    #[test]
    fn ed25519_keytype() {
        let key = PublicKey::parse(TEST_ED25519_KEY).unwrap();
        assert_eq!("ssh-ed25519", key.keytype());
    }

    #[test]
    fn ed25519_fingerprint() {
        let key = PublicKey::parse(TEST_ED25519_KEY).unwrap();
        assert_eq!("SHA256:A/lHzXxsgbp11dcKKfSDyNQIdep7EQgZEoRYVDBfNdI", key.fingerprint());
    }

    #[test]
    fn ed25519_fingerprint_string() {
        let key = PublicKey::parse(TEST_ED25519_KEY).unwrap();
        assert_eq!("256 SHA256:A/lHzXxsgbp11dcKKfSDyNQIdep7EQgZEoRYVDBfNdI demos@siril (ED25519)", key.to_fingerprint_string());
    }

    #[test]
    fn ecdsa256_parse_to_string() {
        let key = PublicKey::parse(TEST_ECDSA256_KEY).unwrap();
        let out = key.to_string();
        assert_eq!(TEST_ECDSA256_KEY, out);
    }

    #[test]
    fn ecdsa256_size() {
        let key = PublicKey::parse(TEST_ECDSA256_KEY).unwrap();
        assert_eq!(256, key.size());
    }

    #[test]
    fn ecdsa256_keytype() {
        let key = PublicKey::parse(TEST_ECDSA256_KEY).unwrap();
        assert_eq!("ecdsa-sha2-nistp256", key.keytype());
    }

    #[test]
    fn ecdsa256_fingerprint() {
        let key = PublicKey::parse(TEST_ECDSA256_KEY).unwrap();
        assert_eq!("SHA256:BzS5YXMW/d2vFk8Oqh+nKmvKr8X/FTLBfJgDGLu5GAs", key.fingerprint());
    }

    #[test]
    fn ecdsa256_fingerprint_string() {
        let key = PublicKey::parse(TEST_ECDSA256_KEY).unwrap();
        assert_eq!("256 SHA256:BzS5YXMW/d2vFk8Oqh+nKmvKr8X/FTLBfJgDGLu5GAs demos@siril (ECDSA)", key.to_fingerprint_string());
    }
}

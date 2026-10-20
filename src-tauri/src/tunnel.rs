//! SSH tunnel — ต่อ database ที่อยู่หลัง bastion / jump host
//! เปิด port ชั่วคราวบน 127.0.0.1 แล้วส่งทุกการเชื่อมต่อผ่าน SSH ไปยัง host:port ของ database
//! (local forwarding แบบ ssh -L) ตัว driver ของ database ต่อเข้า port นั้นเหมือนต่อ database ในเครื่อง
//!
//! host key: ครั้งแรกจำ fingerprint ไว้ (UI เก็บลง connection) ครั้งต่อไปไม่ตรง = ปฏิเสธ
//! กันโดนดักกลางทาง — ถ้าเปลี่ยน server จริงให้ล้าง fingerprint ในฟอร์มแล้วต่อใหม่

use super::{err, R};
use russh::client;
use russh::keys::{load_secret_key, HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[derive(Deserialize, Clone, Debug)]
pub struct Ssh {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// password ของ SSH — ใช้เมื่อไม่มี key_path
    pub password: Option<String>,
    pub key_path: Option<String>,
    pub key_pass: Option<String>,
    /// fingerprint (SHA256) ที่จำไว้ — None = ครั้งแรก
    pub fingerprint: Option<String>,
}

/// tunnel ที่เปิดอยู่ — drop แล้วปิดเอง (หยุดรับการเชื่อมต่อ + ปิด SSH session)
pub struct Tunnel {
    pub port: u16,
    pub fingerprint: String,
    task: JoinHandle<()>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Check {
    expect: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Check {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let fp = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let ok = self.expect.as_ref().map_or(true, |e| *e == fp);
        *self.seen.lock().unwrap() = Some(fp);
        Ok(ok)
    }
}

/// เปิด tunnel ไปยัง host:port ของ database ใน URL แล้วคืน URL ใหม่ที่ชี้มาที่ port ในเครื่อง
pub async fn through(ssh: &Ssh, url: &str) -> R<(String, Tunnel)> {
    let mut u = url::Url::parse(url).map_err(|e| format!("อ่าน connection URL ไม่ได้: {}", e))?;
    let host = u.host_str().ok_or("connection URL ไม่มี host")?.to_string();
    let port = u
        .port()
        .unwrap_or(if u.scheme().starts_with("mysql") || u.scheme() == "mariadb" { 3306 } else { 5432 });
    let t = open(ssh, host, port).await?;
    u.set_host(Some("127.0.0.1")).map_err(err)?;
    u.set_port(Some(t.port)).map_err(|_| "ตั้ง port ใน URL ไม่ได้".to_string())?;
    Ok((u.to_string(), t))
}

pub async fn open(ssh: &Ssh, target_host: String, target_port: u16) -> R<Tunnel> {
    let seen = Arc::new(Mutex::new(None));
    let cfg = Arc::new(client::Config {
        nodelay: true,
        keepalive_interval: Some(std::time::Duration::from_secs(30)),
        ..Default::default()
    });
    let check = Check { expect: ssh.fingerprint.clone().filter(|f| !f.is_empty()), seen: seen.clone() };
    let mut s = match client::connect(cfg, (ssh.host.as_str(), ssh.port), check).await {
        Ok(s) => s,
        Err(e) => {
            let got = seen.lock().unwrap().clone();
            return Err(match (got, &ssh.fingerprint) {
                (Some(got), Some(want)) if !want.is_empty() && got != *want => format!(
                    "SSH host key ของ {} ไม่ตรงกับที่จำไว้ — อาจมีคนดักกลางทาง\nจำไว้: {}\nตอนนี้: {}\nถ้าเปลี่ยน server จริง ให้กดล้าง fingerprint ในฟอร์มแล้วต่อใหม่",
                    ssh.host, want, got
                ),
                _ => format!("ต่อ SSH {}:{} ไม่ได้: {}", ssh.host, ssh.port, e),
            });
        }
    };

    let ok = match ssh.key_path.as_deref().filter(|p| !p.is_empty()) {
        Some(path) => {
            let key = load_secret_key(path, ssh.key_pass.as_deref().filter(|p| !p.is_empty()))
                .map_err(|e| format!("อ่าน private key {} ไม่ได้: {}", path, e))?;
            let hash = s.best_supported_rsa_hash().await.map_err(err)?.flatten();
            s.authenticate_publickey(&ssh.user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(err)?
                .success()
        }
        None => s
            .authenticate_password(&ssh.user, ssh.password.clone().unwrap_or_default())
            .await
            .map_err(err)?
            .success(),
    };
    if !ok {
        return Err(format!("SSH: user {} หรือ password / key ไม่ถูกต้อง", ssh.user));
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(err)?;
    let port = listener.local_addr().map_err(err)?.port();
    let handle = Arc::new(s);
    let task = tokio::spawn(async move {
        while let Ok((mut tcp, peer)) = listener.accept().await {
            let h = handle.clone();
            let host = target_host.clone();
            tokio::spawn(async move {
                if let Ok(ch) = h
                    .channel_open_direct_tcpip(host, target_port as u32, peer.ip().to_string(), peer.port() as u32)
                    .await
                {
                    let mut st = ch.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut st).await;
                }
            });
        }
    });
    let fingerprint = seen.lock().unwrap().clone().unwrap_or_default();
    Ok(Tunnel { port, fingerprint, task })
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::server::{Auth, Msg, Server as _, Session};
    use russh::Channel;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Clone)]
    struct Srv;
    struct H;

    impl russh::server::Server for Srv {
        type Handler = H;
        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> H {
            H
        }
    }

    impl russh::server::Handler for H {
        type Error = russh::Error;

        async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
            Ok(if user == "u" && password == "pw" { Auth::Accept } else { Auth::reject() })
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: Channel<Msg>,
            host: &str,
            port: u32,
            _: &str,
            _: u32,
            reply: russh::server::ChannelOpenHandle,
            _: &mut Session,
        ) -> Result<(), Self::Error> {
            let target = format!("{}:{}", host, port);
            reply.accept().await;
            tokio::spawn(async move {
                if let Ok(mut tcp) = tokio::net::TcpStream::connect(target).await {
                    let mut st = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut st).await;
                }
            });
            Ok(())
        }
    }

    /// SSH server จริง (ในโปรเซสเทสต์) + echo server เป็น "database" ปลายทาง
    async fn servers() -> (u16, u16) {
        let key = russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
        let cfg = Arc::new(russh::server::Config {
            keys: vec![key],
            auth_rejection_time: std::time::Duration::from_millis(10),
            auth_rejection_time_initial: Some(std::time::Duration::from_millis(0)),
            ..Default::default()
        });
        let ssh_sock = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ssh_port = ssh_sock.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = Srv.run_on_socket(cfg, &ssh_sock).await;
        });
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut c, _)) = echo.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = c.read(&mut buf).await {
                        if n == 0 || c.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        (ssh_port, echo_port)
    }

    fn cfg(port: u16, password: &str, fingerprint: Option<String>) -> Ssh {
        Ssh {
            host: "127.0.0.1".into(),
            port,
            user: "u".into(),
            password: Some(password.into()),
            key_path: None,
            key_pass: None,
            fingerprint,
        }
    }

    #[tokio::test]
    async fn forwards_and_pins_host_key() {
        let (ssh_port, echo_port) = servers().await;

        // ส่งข้อมูลผ่าน tunnel ไปถึงปลายทางแล้วได้กลับมาครบ
        let t = open(&cfg(ssh_port, "pw", None), "127.0.0.1".into(), echo_port).await.unwrap();
        assert!(t.fingerprint.starts_with("SHA256:"), "{}", t.fingerprint);
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", t.port)).await.unwrap();
        c.write_all(b"select 1").await.unwrap();
        let mut buf = [0u8; 8];
        c.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"select 1");

        // ต่อซ้ำด้วย fingerprint ที่จำไว้ = ผ่าน, fingerprint ไม่ตรง = ปฏิเสธ
        let fp = t.fingerprint.clone();
        assert!(open(&cfg(ssh_port, "pw", Some(fp)), "127.0.0.1".into(), echo_port).await.is_ok());
        let e = open(&cfg(ssh_port, "pw", Some("SHA256:something-else".into())), "127.0.0.1".into(), echo_port)
            .await
            .err()
            .unwrap();
        assert!(e.contains("ไม่ตรงกับที่จำไว้"), "{}", e);

        // password ผิด
        let e = open(&cfg(ssh_port, "nope", None), "127.0.0.1".into(), echo_port).await.err().unwrap();
        assert!(e.contains("ไม่ถูกต้อง"), "{}", e);

        // drop = ปิด port
        let port = t.port;
        drop(t);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err());
    }

    #[tokio::test]
    async fn url_points_to_local_port() {
        let (ssh_port, echo_port) = servers().await;
        let (url, t) = through(&cfg(ssh_port, "pw", None), &format!("postgres://a:b@127.0.0.1:{}/db?sslmode=require", echo_port))
            .await
            .unwrap();
        assert_eq!(url, format!("postgres://a:b@127.0.0.1:{}/db?sslmode=require", t.port));
    }

    /// Postgres จริงผ่าน tunnel
    #[tokio::test]
    #[ignore]
    async fn postgres_through_tunnel() {
        let base = std::env::var("MARKDB_TEST_PG").expect("ตั้ง MARKDB_TEST_PG ก่อน");
        let (ssh_port, _) = servers().await;
        let (url, _t) = through(&cfg(ssh_port, "pw", None), &format!("{}/postgres", base)).await.unwrap();
        let p = sqlx::PgPool::connect(&url).await.unwrap();
        let (v,): (i32,) = sqlx::query_as("select 41 + 1").fetch_one(&p).await.unwrap();
        assert_eq!(v, 42);
    }
}

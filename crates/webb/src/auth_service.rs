use hyper::{Method, StatusCode};
use tokio::runtime::Builder;
use tokio::sync::mpsc::Sender;

use bytes::Bytes;
use http_body_util::Full;
use hyper::service::Service;
use hyper::{Request, Response, body::Incoming as IncomingBody};

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

static CONFIRM: &[u8] = b"<html><head><title>Telescope login</title><style>body{font-family: monospace;background-color: gray;color: whitesmoke;}</style></head><body><h1>Telescope</h1><p>Logged in!, now you can close this window safely.</p></body></html>";
static NOT_VALID: &[u8] = b"Invalid Request";

#[derive(Debug, Clone)]
pub struct AuthService2 {
    pub tx: Arc<Sender<(String, String)>>,
}

impl Service<Request<IncomingBody>> for AuthService2 {
    type Response = Response<Full<Bytes>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<IncomingBody>) -> Self::Future {

        let res = match (req.method(), req.uri().path()) {
            (&Method::GET, "/login") => {
                let pnq = req.uri().path_and_query();
                if let Some(params) = pnq.unwrap().query() {
                    let mut message: (String, String) = (String::new(), String::new());
                    let parameters = params.split('&').collect::<Vec<&str>>();
                    for param in parameters {
                        let p = param.split('=').collect::<Vec<&str>>();
                        match p[0] {
                            "code" => {
                                message.0 = p[1].to_string();
                            }
                            "state" => {
                                message.1 = p[1].to_string();
                            }
                            _ => (),
                        }
                    }
                    if !message.0.is_empty() && !message.1.is_empty() {
                        let rt = Builder::new_current_thread().enable_all().build().unwrap();
                        let atx = Arc::clone(&self.tx);
                        std::thread::spawn(move || {
                            rt.block_on(async {

                                let _res = atx.send(message).await;
                            });
                        });
                        Ok(Response::builder()
                            .status(StatusCode::OK)
                            .body(Full::new(Bytes::from_static(CONFIRM)))
                            .unwrap())
                    } else {
                        Ok(Response::builder()
                            .status(StatusCode::UNPROCESSABLE_ENTITY)
                            .body(Full::new(Bytes::from_static(NOT_VALID)))
                            .unwrap())
                    }
                } else {
                    Ok(Response::builder()
                        .status(StatusCode::UNPROCESSABLE_ENTITY)
                        .body(Full::new(Bytes::from_static(NOT_VALID)))
                        .unwrap())
                }
            }
            _ => Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from_static(NOT_VALID)))
                .unwrap()),
        };
        Box::pin(async { res })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    /// Spawns a hyper server with the `AuthService2` listening on an
    /// ephemeral port, and returns the address and the receiving end of the
    /// channel where the OAuth parameters are delivered.
    async fn spawn_server() -> (SocketAddr, mpsc::Receiver<(String, String)>) {
        let (tx, rx) = mpsc::channel(1);
        let service = AuthService2 { tx: Arc::new(tx) };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let io = TokioIo::new(stream);
                let service = service.clone();
                tokio::spawn(async move {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .await;
                });
            }
        });
        (addr, rx)
    }

    async fn get(addr: SocketAddr, path_and_query: &str) -> (StatusCode, String) {
        let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
        let uri = format!("http://{}{}", addr, path_and_query)
            .parse()
            .unwrap();
        let response = client.get(uri).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn login_with_code_and_state_is_accepted() {
        let (addr, mut rx) = spawn_server().await;

        let (status, body) = get(addr, "/login?code=auth-code&state=secret-state").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Logged in!"));

        // the OAuth parameters are forwarded through the channel
        let message = timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for the auth message")
            .expect("channel closed unexpectedly");
        assert_eq!(
            message,
            (String::from("auth-code"), String::from("secret-state"))
        );
    }

    #[tokio::test]
    async fn login_accepts_parameters_in_any_order() {
        let (addr, mut rx) = spawn_server().await;

        let (status, _) = get(addr, "/login?state=secret-state&code=auth-code").await;
        assert_eq!(status, StatusCode::OK);

        let message = timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("timed out waiting for the auth message")
            .expect("channel closed unexpectedly");
        assert_eq!(
            message,
            (String::from("auth-code"), String::from("secret-state"))
        );
    }

    #[tokio::test]
    async fn login_without_query_is_rejected() {
        let (addr, _rx) = spawn_server().await;

        let (status, body) = get(addr, "/login").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body, "Invalid Request");
    }

    #[tokio::test]
    async fn login_with_missing_state_is_rejected() {
        let (addr, _rx) = spawn_server().await;

        let (status, body) = get(addr, "/login?code=auth-code").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body, "Invalid Request");
    }

    #[tokio::test]
    async fn login_with_missing_code_is_rejected() {
        let (addr, _rx) = spawn_server().await;

        let (status, body) = get(addr, "/login?state=secret-state").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body, "Invalid Request");
    }

    #[tokio::test]
    async fn unknown_path_returns_not_found() {
        let (addr, _rx) = spawn_server().await;

        let (status, body) = get(addr, "/nowhere").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, "Invalid Request");
    }
}

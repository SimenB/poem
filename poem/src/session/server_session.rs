use std::sync::Arc;

use rand::{distributions::Alphanumeric, rngs::OsRng, Rng};

use crate::{
    middleware::{CookieJarManager, CookieJarManagerEndpoint},
    session::{session_storage::SessionStorage, CookieConfig, Session, SessionStatus},
    Endpoint, Middleware, Request, Result,
};

/// A middleware for server-side session.
pub struct ServerSession<T> {
    config: Arc<CookieConfig>,
    storage: Arc<T>,
}

impl<T> ServerSession<T> {
    /// Create a `ServerSession` middleware.
    pub fn new(config: CookieConfig, storage: T) -> Self {
        Self {
            config: Arc::new(config),
            storage: Arc::new(storage),
        }
    }
}

impl<T: SessionStorage, E: Endpoint> Middleware<E> for ServerSession<T> {
    type Output = CookieJarManagerEndpoint<ServerSessionEndpoint<T, E>>;

    fn transform(&self, ep: E) -> Self::Output {
        CookieJarManager::new().transform(ServerSessionEndpoint {
            inner: ep,
            config: self.config.clone(),
            storage: self.storage.clone(),
        })
    }
}

fn generate_session_id() -> String {
    let value = std::iter::repeat(())
        .map(|()| OsRng.sample(Alphanumeric))
        .take(32)
        .collect::<Vec<_>>();
    String::from_utf8(value).unwrap_or_default()
}

/// Endpoint for `ServerSession` middleware.
pub struct ServerSessionEndpoint<T, E> {
    inner: E,
    config: Arc<CookieConfig>,
    storage: Arc<T>,
}

#[async_trait::async_trait]
impl<T: SessionStorage, E: Endpoint> Endpoint for ServerSessionEndpoint<T, E> {
    type Output = Result<E::Output>;

    async fn call(&self, mut req: Request) -> Self::Output {
        let cookie_jar = req.cookie().clone();
        let session_id = self.config.get_cookie_value(&cookie_jar);
        let session = match &session_id {
            Some(session_id) => {
                let entries = self.storage.load_session(session_id).await?;
                Session::new(entries)
            }
            None => Session::default(),
        };

        req.extensions_mut().insert(session.clone());
        let resp = self.inner.call(req).await;

        match session.status() {
            SessionStatus::Changed => match session_id {
                Some(session_id) => {
                    self.storage
                        .update_session(&session_id, &session.entries(), self.config.ttl())
                        .await?;
                }
                None => {
                    let session_id = generate_session_id();
                    self.config.set_cookie_value(&cookie_jar, &session_id);
                    self.storage
                        .update_session(&session_id, &session.entries(), self.config.ttl())
                        .await?;
                }
            },
            SessionStatus::Renewed => {
                if let Some(session_id) = session_id {
                    self.storage.remove_session(&session_id).await?;
                }

                let session_id = generate_session_id();
                self.config.set_cookie_value(&cookie_jar, &session_id);
                self.storage
                    .update_session(&session_id, &session.entries(), self.config.ttl())
                    .await?;
            }
            SessionStatus::Purged => {
                if let Some(session_id) = session_id {
                    self.storage.remove_session(&session_id).await?;
                    self.config.remove_cookie(&cookie_jar);
                }
            }
            SessionStatus::Unchanged => {}
        };

        Ok(resp)
    }
}
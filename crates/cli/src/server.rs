use std::net::SocketAddr;
use std::sync::Arc;

use platform_api::{router, AppState, Clock, SqliteDirectory};
use platform_store::SqliteStore;
use platform_web::WebState;
use tokio::net::TcpListener;

use crate::error::CliError;
use crate::GRACE_SECS;

pub async fn serve(
    listener: TcpListener,
    store: SqliteStore,
    clock: Clock,
) -> Result<(), CliError> {
    let state = AppState::new(Arc::new(SqliteDirectory::new(store)), clock, GRACE_SECS);
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

// L'API des bots (jeton par container) et le dashboard (session par cookie) partagent le
// même port mais aucun état : chacun a sa connexion à la base.
pub async fn serve_all(
    listener: TcpListener,
    api_store: SqliteStore,
    web: WebState,
    clock: Clock,
) -> Result<(), CliError> {
    let api = AppState::new(Arc::new(SqliteDirectory::new(api_store)), clock, GRACE_SECS);
    let app = router(api).merge(platform_web::router(web));
    // ConnectInfo permet d'extraire l'IP réelle du client pour le rate limiting.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

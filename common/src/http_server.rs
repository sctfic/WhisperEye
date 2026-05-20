use esp_idf_svc::http::server::{EspHttpServer, Configuration, Request, Response};
use esp_idf_svc::http::Method;
use totp_rs::{TOTP, Algorithm, Secret};
use log::{info, warn, error};
use std::sync::{Arc, Mutex};
use anyhow::Result;

pub struct HttpServerManager {
    server: EspHttpServer<'static>,
    totp_secret: Arc<Mutex<String>>,
}

impl HttpServerManager {
    /// Initialise et démarre le serveur HTTP sur le port spécifié
    pub fn new(port: u16, default_totp_secret: &str) -> Result<Self> {
        info!("Démarrage du serveur HTTP sur le port {}...", port);
        
        let config = Configuration {
            http_port: port,
            ..Default::default()
        };
        
        let server = EspHttpServer::new(&config)?;
        let totp_secret = Arc::new(Mutex::new(default_totp_secret.to_string()));

        let mut manager = Self { server, totp_secret };
        manager.register_endpoints()?;

        Ok(manager)
    }

    /// Enregistre les endpoints de l'API web de base
    fn register_endpoints(&mut self) -> Result<()> {
        let totp_secret_clone = self.totp_secret.clone();

        // 1. Endpoint d'accueil public
        self.server.fn_handler("/", Method::Get, move |_req| {
            let html = r#"
                <!DOCTYPE html>
                <html>
                <head>
                    <title>WhisperEye Node</title>
                    <meta charset="utf-8">
                    <meta name="viewport" content="width=device-width, initial-scale=1">
                    <style>
                        body { font-family: sans-serif; background: #121214; color: #e1e1e6; text-align: center; padding: 2rem; }
                        h1 { color: #00b4d8; }
                        .container { max-width: 500px; margin: auto; background: #202024; padding: 2rem; border-radius: 8px; box-shadow: 0 4px 8px rgba(0,0,0,0.2); }
                        input, button { padding: 0.5rem; width: 80%; margin: 0.5rem 0; border-radius: 4px; border: 1px solid #323238; background: #121214; color: #fff; }
                        button { background: #00b4d8; color: #121214; font-weight: bold; cursor: pointer; border: none; }
                        button:hover { background: #90e0ef; }
                    </style>
                </head>
                <body>
                    <div class="container">
                        <h1>WhisperEye Admin Panel</h1>
                        <p>Connexion sécurisée requise via TOTP.</p>
                        <form action="/admin" method="GET">
                            <input type="text" name="token" placeholder="Code TOTP (6 chiffres)" required maxlength="6" pattern="[0-9]{6}">
                            <button type="submit">Valider et Entrer</button>
                        </form>
                    </div>
                </body>
                </html>
            "#;
            
            let mut response = _req.into_ok_response()?;
            response.write(html.as_bytes())?;
            Ok::<(), esp_idf_svc::sys::EspError>(())
        })?;

        // 2. Endpoint sécurisé par TOTP
        self.server.fn_handler("/admin", Method::Get, move |req| {
            let uri = req.uri();
            let mut token = "";
            
            // Extraction rudimentaire du paramètre "token" dans l'URI (?token=XXXXXX)
            if let Some(pos) = uri.find("token=") {
                let val = &uri[pos + 6..];
                token = val.split('&').next().unwrap_or("");
            }

            let secret_str = totp_secret_clone.lock().unwrap();
            let mut response = req.into_ok_response()?;

            // Validation du code TOTP
            if Self::validate_totp(token, &secret_str) {
                info!("Code TOTP valide fourni. Accès au panneau d'administration autorisé.");
                let admin_html = r#"
                    <!DOCTYPE html>
                    <html>
                    <head>
                        <title>WhisperEye - Admin</title>
                        <style>
                            body { font-family: sans-serif; background: #121214; color: #e1e1e6; padding: 2rem; }
                            h1 { color: #00f5d4; }
                            .card { background: #202024; padding: 1.5rem; border-radius: 8px; margin-bottom: 1rem; }
                        </style>
                    </head>
                    <body>
                        <h1>Panneau d'Administration WhisperEye</h1>
                        <div class="card">
                            <h3>Statut du Système</h3>
                            <p>Toutes les sondes sont en cours d'exécution.</p>
                        </div>
                    </body>
                    </html>
                "#;
                response.write(admin_html.as_bytes())?;
            } else {
                warn!("Tentative d'accès non autorisée avec un jeton TOTP invalide.");
                let unauthorized_html = r#"
                    <html>
                    <body style="background: #121214; color: #ff6b6b; text-align: center; padding-top: 10%;">
                        <h1>Accès Refusé</h1>
                        <p>Le code TOTP fourni est invalide ou expiré.</p>
                        <a href="/" style="color: #00b4d8;">Réessayer</a>
                    </body>
                    </html>
                "#;
                response.write(unauthorized_html.as_bytes())?;
            }
            
            Ok::<(), esp_idf_svc::sys::EspError>(())
        })?;

        Ok(())
    }

    /// Valide un code TOTP reçu
    fn validate_totp(token: &str, secret_str: &str) -> bool {
        if token.len() != 6 {
            return false;
        }

        // Crée un objet TOTP à partir de la clé secrète. Si ce n'est pas du Base32 valide,
        // on retombe sur du texte brut.
        let secret_bytes = match Secret::Encoded(secret_str.to_string()).to_bytes() {
            Ok(bytes) => bytes,
            Err(_) => {
                // Secret brut converti en bytes
                Secret::Raw(secret_str.as_bytes().to_vec()).to_bytes().unwrap_or_default()
            }
        };

        if !secret_bytes.is_empty() {
            if let Ok(totp) = TOTP::new(
                Algorithm::SHA1,
                6,
                1,
                30,
                secret_bytes,
                Some("Sctfic".to_string()),
                "WhisperEye".to_string(),
            ) {
                // Vérifie la validité du jeton par rapport à l'heure système actuelle de l'ESP32
                if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                    return totp.check(token, now.as_secs());
                }
            }
        }
        false
    }

    /// Met à jour dynamiquement le secret TOTP stocké
    pub fn update_secret(&self, new_secret: &str) {
        let mut secret = self.totp_secret.lock().unwrap();
        *secret = new_secret.to_string();
        info!("Secret TOTP mis à jour dynamiquement.");
    }
}

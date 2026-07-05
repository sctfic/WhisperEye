use esp_idf_svc::http::server::EspHttpServer;
use std::sync::{Arc, Mutex};
use std::thread;
use crate::wifi::{NetManager, NetState};
use common::nvs_storage::NvsStorage;
use crate::actuators::{Actuators, ActuatorsState, ScheduledActions};
use crate::board::Board;
use crate::i2c::I2c;
use crate::MeshState;
use crate::UrlValidationState;
use crate::web_handlers;

trait EspHttpServerApiExt {
    fn api_handler<F>(&mut self, uri: &str, method: esp_idf_svc::http::Method, handler: F) -> Result<(), anyhow::Error>
    where
        F: Fn(esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection<'_>>) -> Result<(), anyhow::Error> + Send + Sync + 'static;
}

impl EspHttpServerApiExt for EspHttpServer<'_> {
    fn api_handler<F>(&mut self, uri: &str, method: esp_idf_svc::http::Method, handler: F) -> Result<(), anyhow::Error>
    where
        F: Fn(esp_idf_svc::http::server::Request<&mut esp_idf_svc::http::server::EspHttpConnection<'_>>) -> Result<(), anyhow::Error> + Send + Sync + 'static,
    {
        self.fn_handler(uri, method, move |req| {
            crate::wifi::API_DOWNLOAD_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
            let res = handler(req);
            crate::wifi::API_DOWNLOAD_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
            crate::wifi::API_UPLOAD_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
            res
        })?;
        Ok(())
    }
}

pub fn register_routes(
    server: &mut EspHttpServer,
    nvs_storage: Arc<Mutex<NvsStorage>>,
    wifi_manager: Arc<Mutex<NetManager>>,
    mesh_state: Arc<Mutex<MeshState>>,
    url_validation_state: Arc<Mutex<UrlValidationState>>,
    discovered_probes: Arc<Vec<String>>,
    actuators_state: Arc<Mutex<ActuatorsState>>,
    actuators: Arc<Mutex<Actuators>>,
    scheduled_actions: Arc<Mutex<ScheduledActions>>,
    board: Arc<Mutex<Board>>,
    i2c: Arc<Mutex<I2c>>,
    cron_handle: crate::cron::CronHandle,
) -> Result<(), anyhow::Error> {
    log::info!("Registering HTTP routes in route.rs...");

    // GET /favicon.ico
    server.fn_handler("/favicon.ico", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        web_handlers::handle_favicon(req)
    })?;

    // GET /
    server.fn_handler("/", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_ok_response()?;
        response.write(crate::web_pages::PRODUCTION_HTML.as_bytes())?;
        Ok(())
    })?;

    // GET /proxy
    let proxy_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/proxy", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        web_handlers::handle_proxy(req, Arc::clone(&proxy_mesh))
    })?;

    // Captive Portal Redirects
    server.fn_handler("/generate_204", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        web_handlers::handle_captive_redirect(req)
    })?;
    server.fn_handler("/hotspot-detect.html", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        web_handlers::handle_captive_redirect(req)
    })?;
    server.fn_handler("/ncsi.txt", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        web_handlers::handle_captive_redirect(req)
    })?;
    server.fn_handler("/connecttest.txt", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        web_handlers::handle_captive_redirect(req)
    })?;

    // GET /api/network/knowledge
    let nvs_sync = Arc::clone(&nvs_storage);
    let mesh_state_sync = Arc::clone(&mesh_state);
    server.api_handler("/api/network/knowledge", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        web_handlers::handle_network_knowledge(req, Arc::clone(&nvs_sync), Arc::clone(&mesh_state_sync))
    })?;

    // GET /api/status
    let status_nvs = Arc::clone(&nvs_storage);
    let status_wifi = Arc::clone(&wifi_manager);
    let status_mesh = Arc::clone(&mesh_state);
    let status_url = Arc::clone(&url_validation_state);
    server.api_handler("/api/status", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        web_handlers::handle_api_status(req, Arc::clone(&status_nvs), Arc::clone(&status_wifi), Arc::clone(&status_mesh), Arc::clone(&status_url))
    })?;

    // GET /api/capacity
    let cap_nvs = Arc::clone(&nvs_storage);
    let cap_probes = Arc::clone(&discovered_probes);
    let cap_act_state = Arc::clone(&actuators_state);
    let cap_act = Arc::clone(&actuators);
    let cap_sched = Arc::clone(&scheduled_actions);
    let cap_board = Arc::clone(&board);
    let cap_i2c = Arc::clone(&i2c);
    server.api_handler("/api/capacity", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        web_handlers::handle_api_capacity(
            req,
            Arc::clone(&cap_nvs),
            Arc::clone(&cap_probes),
            Arc::clone(&cap_act_state),
            Arc::clone(&cap_act),
            Arc::clone(&cap_sched),
            Arc::clone(&cap_board),
            Arc::clone(&cap_i2c),
        )
    })?;

    // GET /api/check_updates
    let updates_nvs = Arc::clone(&nvs_storage);
    let updates_url = Arc::clone(&url_validation_state);
    server.api_handler("/api/check_updates", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        web_handlers::handle_check_updates(req, Arc::clone(&updates_nvs), Arc::clone(&updates_url))
    })?;

    // GET /api/history
    let cron_history = cron_handle.clone();
    let history_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/history", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let mut state = history_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        let history = cron_history.get_sensor_history();
        let response_data = serde_json::to_string(&history)?;
        let mut response = req.into_response(200, Some("OK"), &[
            ("Content-Type", "application/json"),
            ("Access-Control-Allow-Origin", "*")
        ])?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /api/ssids
    let ssids_wifi = Arc::clone(&wifi_manager);
    let ssids_nvs = Arc::clone(&nvs_storage);
    let ssids_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/ssids", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let mut state = ssids_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        web_handlers::handle_api_ssids(req, Arc::clone(&ssids_wifi), Arc::clone(&ssids_nvs))
    })?;

    // GET /api/sensors
    let sensors_nvs = Arc::clone(&nvs_storage);
    let sensors_i2c = Arc::clone(&i2c);
    let sensors_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/sensors", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let mut state = sensors_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        web_handlers::handle_api_sensors(req, Arc::clone(&sensors_nvs), Arc::clone(&sensors_i2c))
    })?;

    // GET /api/peripherals
    let periphs_nvs = Arc::clone(&nvs_storage);
    let periphs_act_state = Arc::clone(&actuators_state);
    let periphs_probes = Arc::clone(&discovered_probes);
    let periphs_sched = Arc::clone(&scheduled_actions);
    let periphs_board = Arc::clone(&board);
    let periphs_i2c = Arc::clone(&i2c);
    let periphs_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/peripherals", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let mut state = periphs_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        web_handlers::handle_api_peripherals(
            req,
            Arc::clone(&periphs_nvs),
            Arc::clone(&periphs_act_state),
            Arc::clone(&periphs_probes),
            Arc::clone(&periphs_sched),
            Arc::clone(&periphs_board),
            Arc::clone(&periphs_i2c),
        )
    })?;

    // POST /api/peripherals
    let rename_nvs = Arc::clone(&nvs_storage);
    let rename_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/peripherals", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        let mut state = rename_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        web_handlers::handle_rename_peripherals(req, Arc::clone(&rename_nvs))
    })?;

    // POST /api/sensor-correction
    let corr_nvs = Arc::clone(&nvs_storage);
    let corr_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/sensor-correction", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        let mut state = corr_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        web_handlers::handle_sensor_correction(req, Arc::clone(&corr_nvs))
    })?;

    // POST /api/actuators
    let act_state_post = Arc::clone(&actuators_state);
    let act_post = Arc::clone(&actuators);
    let act_nvs = Arc::clone(&nvs_storage);
    let act_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/actuators", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        let mut state = act_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        web_handlers::handle_post_actuators(req, Arc::clone(&act_state_post), Arc::clone(&act_post), Arc::clone(&act_nvs))
    })?;

    // POST /api/actuators/control
    let ctrl_nvs = Arc::clone(&nvs_storage);
    let ctrl_act_state = Arc::clone(&actuators_state);
    let ctrl_act = Arc::clone(&actuators);
    let ctrl_sched = Arc::clone(&scheduled_actions);
    let ctrl_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/actuators/control", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        let mut state = ctrl_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        web_handlers::handle_actuators_control(
            req,
            Arc::clone(&ctrl_nvs),
            Arc::clone(&ctrl_act_state),
            Arc::clone(&ctrl_act),
            Arc::clone(&ctrl_sched),
        )
    })?;

    // POST /api/identify
    let id_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/identify", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        let mut state = id_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        let stop_utc = common::led::extend_identify(15);
        let remaining = common::led::identify_remaining_secs();
        let stop_str = format!("{:?}", stop_utc);
        let json = serde_json::json!({
            "status": "ok",
            "identify_stop_utc": stop_str,
            "identify_remaining_secs": remaining,
        });
        let response_data = serde_json::to_string(&json)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // POST /api/identify/stop
    let id_stop_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/identify/stop", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        let mut state = id_stop_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        common::led::cancel_identify();
        let json = serde_json::json!({"status": "ok", "identify_remaining_secs": 0});
        let response_data = serde_json::to_string(&json)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // POST /api/restart
    let restart_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/restart", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        let mut state = restart_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        let json = serde_json::json!({"status": "ok", "message": "Redémarrage immédiat..."});
        let response_data = serde_json::to_string(&json)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        let _ = thread::Builder::new()
            .name("api_restart_worker".to_string())
            .spawn(|| {
                thread::sleep(std::time::Duration::from_millis(500));
                unsafe { esp_idf_sys::esp_restart(); }
            });
        Ok(())
    })?;

    // POST /api/clear-totp
    let clear_totp_nvs = Arc::clone(&nvs_storage);
    let clear_totp_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/clear-totp", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        let mut state = clear_totp_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        web_handlers::handle_clear_totp(req, Arc::clone(&clear_totp_nvs))
    })?;

    // POST /api/reset
    let reset_nvs = Arc::clone(&nvs_storage);
    let reset_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/reset", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        let mut state = reset_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        web_handlers::handle_reset(req, Arc::clone(&reset_nvs))
    })?;

    // POST /api/network/ApPairing
    let pair_wifi = Arc::clone(&wifi_manager);
    let pair_mesh = Arc::clone(&mesh_state);
    server.api_handler("/api/network/ApPairing", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        {
            let mut net = pair_wifi.lock().unwrap();
            net.state = NetState::ApPairing;
            net.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
            let _ = net.setup_provisioning_ap();
            let mut state = pair_mesh.lock().unwrap();
            state.pairing_until = net.pairing_until;
        }
        let mut response = req.into_ok_response()?;
        response.write(b"Provisioning mode enabled for 120 seconds")?;
        Ok(())
    })?;

    // POST /api/config
    let config_nvs = Arc::clone(&nvs_storage);
    let config_wifi = Arc::clone(&wifi_manager);
    let config_mesh = Arc::clone(&mesh_state);
    let config_url = Arc::clone(&url_validation_state);
    server.api_handler("/api/config", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        let mut state = config_mesh.lock().unwrap();
        if state.pairing_until.is_some() {
            state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
        }
        drop(state);
        web_handlers::handle_config(req, Arc::clone(&config_nvs), Arc::clone(&config_wifi), Arc::clone(&config_url))
    })?;

    Ok(())
}

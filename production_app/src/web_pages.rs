pub const PRODUCTION_HTML: &str = r#"<!DOCTYPE html>
<html lang="fr">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>WhisperEye - Dashboard Production</title>
    <style>
        :root {
            --bg-color: #0b0f19;
            --card-bg: #151d30;
            --accent-color: #10b981;
            --accent-hover: #059669;
            --secondary-accent: #3b82f6;
            --text-primary: #f1f5f9;
            --text-secondary: #64748b;
            --border-color: #1e293b;
            --danger: #ef4444;
            --warning: #f59e0b;
        }

        * {
            box-sizing: border-box;
            margin: 0;
            padding: 0;
            font-family: 'Segoe UI', Tahoma, Geneva, Verdana, sans-serif;
        }

        body {
            background-color: var(--bg-color);
            color: var(--text-primary);
            min-height: 100vh;
            display: flex;
            flex-direction: column;
            align-items: center;
            padding: 2rem 1rem;
        }

        header {
            text-align: center;
            margin-bottom: 2rem;
            max-width: 650px;
            width: 100%;
        }

        h1 {
            font-size: 2.2rem;
            font-weight: 800;
            background: linear-gradient(135deg, #10b981 0%, #3b82f6 100%);
            -webkit-background-clip: text;
            -webkit-text-fill-color: transparent;
            margin-bottom: 0.5rem;
        }

        header p {
            color: var(--text-secondary);
            font-size: 1rem;
        }

        .container {
            width: 100%;
            max-width: 650px;
            background-color: var(--card-bg);
            border: 1px solid var(--border-color);
            border-radius: 16px;
            box-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.4);
            overflow: hidden;
        }

        .tabs {
            display: flex;
            background-color: rgba(11, 15, 25, 0.6);
            border-bottom: 1px solid var(--border-color);
        }

        .tab-btn {
            flex: 1;
            padding: 1rem;
            background: none;
            border: none;
            color: var(--text-secondary);
            font-weight: 600;
            font-size: 0.95rem;
            cursor: pointer;
            transition: all 0.3s ease;
            text-align: center;
            border-bottom: 2px solid transparent;
        }

        .tab-btn:hover {
            color: var(--text-primary);
            background-color: rgba(255, 255, 255, 0.02);
        }

        .tab-btn.active {
            color: var(--accent-color);
            border-bottom-color: var(--accent-color);
            background-color: rgba(255, 255, 255, 0.04);
        }

        .tab-content {
            padding: 2rem;
            display: none;
        }

        .tab-content.active {
            display: block;
        }

        .info-group {
            display: flex;
            justify-content: space-between;
            padding: 0.8rem 0;
            border-bottom: 1px solid rgba(30, 41, 59, 0.6);
        }

        .info-group:last-child {
            border-bottom: none;
        }

        .info-label {
            color: var(--text-secondary);
            font-weight: 500;
        }

        .info-value {
            font-weight: 600;
            color: var(--text-primary);
        }

        .badge {
            padding: 0.25rem 0.6rem;
            border-radius: 9999px;
            font-size: 0.8rem;
            font-weight: 600;
        }

        .badge-green {
            background-color: rgba(16, 185, 129, 0.15);
            color: var(--accent-color);
        }

        .badge-blue {
            background-color: rgba(59, 130, 246, 0.15);
            color: var(--secondary-accent);
        }

        .badge-warn {
            background-color: rgba(245, 158, 11, 0.15);
            color: var(--warning, #f59e0b);
        }

        .badge-error {
            background-color: rgba(239, 68, 68, 0.15);
            color: var(--danger, #ef4444);
        }

        /* GRID FOR SENSORS & ACTUATORS */
        .dashboard-grid {
            display: grid;
            grid-template-columns: 1fr 1fr;
            gap: 1.5rem;
            margin-bottom: 1.5rem;
        }

        @media (max-width: 500px) {
            .dashboard-grid {
                grid-template-columns: 1fr;
            }
        }

        .widget {
            background-color: rgba(11, 15, 25, 0.4);
            border: 1px solid var(--border-color);
            border-radius: 12px;
            padding: 1.25rem;
            display: flex;
            flex-direction: column;
        }

        .widget-title {
            color: var(--text-secondary);
            font-size: 0.85rem;
            font-weight: 600;
            text-transform: uppercase;
            letter-spacing: 0.05em;
            margin-bottom: 0.5rem;
        }

        .widget-value {
            font-size: 2rem;
            font-weight: 700;
            color: var(--text-primary);
            margin: 0.5rem 0;
        }

        .widget-unit {
            font-size: 1rem;
            color: var(--text-secondary);
            font-weight: 500;
        }

        /* SWITCH FOR RELAYS */
        .switch-container {
            display: flex;
            align-items: center;
            justify-content: space-between;
            margin-top: 1rem;
            padding: 0.5rem 0;
        }

        .switch {
            position: relative;
            display: inline-block;
            width: 48px;
            height: 24px;
        }

        .switch input {
            opacity: 0;
            width: 0;
            height: 0;
        }

        .slider {
            position: absolute;
            cursor: pointer;
            top: 0;
            left: 0;
            right: 0;
            bottom: 0;
            background-color: #334155;
            transition: .3s;
            border-radius: 24px;
        }

        .slider:before {
            position: absolute;
            content: "";
            height: 18px;
            width: 18px;
            left: 3px;
            bottom: 3px;
            background-color: white;
            transition: .3s;
            border-radius: 50%;
        }

        input:checked + .slider {
            background-color: var(--accent-color);
        }

        input:checked + .slider:before {
            transform: translateX(24px);
        }

        /* SLIDER FOR PWM */
        .slider-group {
            margin-top: 1rem;
        }

        .range-slider {
            -webkit-appearance: none;
            width: 100%;
            height: 6px;
            border-radius: 5px;
            background: #334155;
            outline: none;
            margin: 1rem 0;
        }

        .range-slider::-webkit-slider-thumb {
            -webkit-appearance: none;
            appearance: none;
            width: 18px;
            height: 18px;
            border-radius: 50%;
            background: var(--secondary-accent);
            cursor: pointer;
            transition: transform 0.1s;
        }

        .range-slider::-webkit-slider-thumb:hover {
            transform: scale(1.2);
        }

        /* FORM STYLING */
        .form-group {
            margin-bottom: 1.5rem;
        }

        label {
            display: block;
            margin-bottom: 0.5rem;
            color: var(--text-secondary);
            font-weight: 500;
            font-size: 0.9rem;
        }

        input, select {
            width: 100%;
            padding: 0.75rem 1rem;
            background-color: rgba(11, 15, 25, 0.6);
            border: 1px solid var(--border-color);
            border-radius: 8px;
            color: var(--text-primary);
            font-size: 1rem;
            outline: none;
            transition: border-color 0.2s;
        }

        input:focus, select:focus {
            border-color: var(--accent-color);
        }

        .btn {
            width: 100%;
            padding: 0.85rem 1.5rem;
            background-color: var(--accent-color);
            color: #0b0f19;
            font-weight: 700;
            font-size: 1rem;
            border: none;
            border-radius: 8px;
            cursor: pointer;
            transition: all 0.2s;
        }

        .btn:hover {
            background-color: var(--accent-hover);
        }

        .status-message {
            margin-top: 1rem;
            padding: 0.75rem 1rem;
            border-radius: 8px;
            font-size: 0.9rem;
            display: none;
        }

        .status-success {
            background-color: rgba(16, 185, 129, 0.15);
            border: 1px solid var(--accent-color);
            color: var(--accent-color);
            display: block;
        }

        .status-error {
            background-color: rgba(239, 68, 68, 0.15);
            border: 1px solid var(--danger);
            color: var(--danger);
            display: block;
        }

        .pulse {
            display: inline-block;
            width: 8px;
            height: 8px;
            border-radius: 50%;
            background-color: var(--accent-color);
            box-shadow: 0 0 0 0 rgba(16, 185, 129, 0.7);
            animation: pulse 1.5s infinite;
            margin-right: 0.5rem;
        }

        @keyframes pulse {
            0% {
                transform: scale(0.95);
                box-shadow: 0 0 0 0 rgba(16, 185, 129, 0.7);
            }
            70% {
                transform: scale(1);
                box-shadow: 0 0 0 6px rgba(16, 185, 129, 0);
            }
            100% {
                transform: scale(0.95);
                box-shadow: 0 0 0 0 rgba(16, 185, 129, 0);
            }
        }
    </style>
</head>
<body>
    <header>
        <h1>WhisperEye</h1>
        <p><span class="pulse"></span>Firmware Applicatif de Production (Production App)</p>
    </header>

    <div class="container">
        <div class="tabs">
            <button class="tab-btn active" onclick="switchTab('sensors-tab')">Capteurs & Contrôle</button>
            <button class="tab-btn" onclick="switchTab('status-tab')">Statut ESP32</button>
            <button class="tab-btn" onclick="switchTab('config-tab')">Configuration</button>
        </div>

        <!-- TAB 1: SENSORS & ACTUATORS -->
        <div id="sensors-tab" class="tab-content active">
            <h3 style="margin-bottom: 1.25rem; font-size: 1.1rem; border-left: 3px solid var(--accent-color); padding-left: 0.5rem;">
                Relevés Capteurs en Temps Réel
            </h3>
            <div class="dashboard-grid">
                <div class="widget">
                    <span class="widget-title">Température (SHT45)</span>
                    <span class="widget-value"><span id="val_temp_sht">--.-</span><span class="widget-unit">°C</span></span>
                </div>
                <div class="widget">
                    <span class="widget-title">Humidité (SHT45)</span>
                    <span class="widget-value"><span id="val_humi_sht">--.-</span><span class="widget-unit">%</span></span>
                </div>
                <div class="widget">
                    <span class="widget-title">CO2 (SCD41)</span>
                    <span class="widget-value"><span id="val_co2">----</span><span class="widget-unit"> ppm</span></span>
                </div>
                <div class="widget">
                    <span class="widget-title">Température (DS18B20)</span>
                    <span class="widget-value"><span id="val_temp_ds">--.-</span><span class="widget-unit">°C</span></span>
                </div>
            </div>

            <h3 style="margin-bottom: 1.25rem; font-size: 1.1rem; border-left: 3px solid var(--secondary-accent); padding-left: 0.5rem; margin-top: 2rem;">
                Contrôle Actionneurs
            </h3>
            
            <div class="widget" style="margin-bottom: 1rem;">
                <div class="switch-container">
                    <div>
                        <div style="font-weight: 600; font-size: 0.95rem;">Relais de Puissance 1</div>
                        <div style="font-size: 0.8rem; color: var(--text-secondary);">Contrôle de la ligne d'alimentation secondaire</div>
                    </div>
                    <label class="switch">
                        <input type="checkbox" id="relay_1_check" onchange="toggleRelay()">
                        <span class="slider"></span>
                    </label>
                </div>
            </div>

            <div class="widget">
                <div class="slider-group">
                    <div style="display: flex; justify-content: space-between; font-weight: 600; font-size: 0.95rem;">
                        <span>Intensité Lumineuse PWM</span>
                        <span id="pwm_val_badge" style="color: var(--secondary-accent);">0%</span>
                    </div>
                    <div style="font-size: 0.8rem; color: var(--text-secondary); margin-top: 0.15rem;">
                        Contrôle de la sortie LED graduée
                    </div>
                    <input type="range" class="range-slider" id="pwm_slider" min="0" max="100" value="0" onchange="updatePwm(this.value)" oninput="document.getElementById('pwm_val_badge').innerText = this.value + '%'">
                </div>
            </div>
        </div>

        <!-- TAB 2: STATUS -->
        <div id="status-tab" class="tab-content">
            <div class="info-group">
                <span class="info-label">État Système</span>
                <span class="info-value"><span id="system_status_badge" class="badge badge-green">🟢 Opérationnel</span></span>
            </div>
            <div class="info-group">
                <span class="info-label">Mode Réseau</span>
                <span class="info-value"><span id="network_mode" class="badge badge-green">Station</span></span>
            </div>
            <div class="info-group">
                <span class="info-label">SSID Wi-Fi</span>
                <span class="info-value" id="wifi_ssid">Chargement...</span>
            </div>
            <div class="info-group">
                <span class="info-label">Force Signal (RSSI)</span>
                <span class="info-value" id="wifi_rssi">- dBm</span>
            </div>
            <div class="info-group">
                <span class="info-label">Adresse IP</span>
                <span class="info-value" id="ip_addr">0.0.0.0</span>
            </div>
            <div class="info-group">
                <span class="info-label">Passerelle (Gateway)</span>
                <span class="info-value" id="gateway_addr">0.0.0.0</span>
            </div>
            <div class="info-group">
                <span class="info-label">Heure Système</span>
                <span class="info-value" id="sys_time">1970-01-01 00:00:00</span>
            </div>
            <div class="info-group">
                <span class="info-label">NTP Serveur</span>
                <span class="info-value" id="ntp_server">wrt.lan</span>
            </div>
            <div class="info-group">
                <span class="info-label">Firmware Production Actif</span>
                <span class="info-value" id="prod_version">v1.0.0-poc</span>
            </div>
            <div class="info-group">
                <span class="info-label">Dernière OTA Success</span>
                <span class="info-value" id="last_ota">1970-01-01 00:00:00</span>
            </div>
        </div>

        <!-- TAB 3: CONFIGURATION (WITH REBOOT ON SAVE) -->
        <div id="config-tab" class="tab-content">
            <div class="form-group">
                <label for="ssid_select">Réseau Wi-Fi (SSID)</label>
                <select id="ssid_select" onchange="toggleCustomSsid()">
                    <option value="custom">-- Saisir un SSID --</option>
                </select>
            </div>
            
            <div class="form-group" id="ssid_custom_group" style="display: block;">
                <label for="ssid_custom">SSID Wi-Fi Personnalisé</label>
                <input type="text" id="ssid_custom" placeholder="MonSSIDPerso">
            </div>

            <div class="form-group">
                <label for="password">Mot de passe</label>
                <div style="position: relative; display: flex; align-items: center;">
                    <input type="password" id="password" placeholder="••••••••" style="padding-right: 2.5rem;">
                    <span id="toggle_password_icon" onclick="togglePasswordVisibility()" style="position: absolute; right: 12px; cursor: pointer; font-size: 1.2rem; user-select: none;">👁️</span>
                </div>
            </div>

            <div class="form-group">
                <label for="update_url">URL de Mise à Jour (Production)</label>
                <input type="text" id="update_url" value="">
            </div>

            <button class="btn" onclick="saveConfiguration()">Sauvegarder & Redémarrer l'ESP32</button>
            <div id="config_status" class="status-message"></div>
        </div>
    </div>

    <script>
        function switchTab(tabId) {
            document.querySelectorAll('.tab-content').forEach(tab => tab.classList.remove('active'));
            document.querySelectorAll('.tab-btn').forEach(btn => btn.classList.remove('active'));
            
            document.getElementById(tabId).classList.add('active');
            event.currentTarget.classList.add('active');

            if (tabId === 'status-tab') {
                fetchStatus();
            } else if (tabId === 'config-tab') {
                scanSsids();
            }
        }

        function togglePasswordVisibility() {
            const psk = document.getElementById('password');
            const toggleIcon = document.getElementById('toggle_password_icon');
            if (psk.type === 'password') {
                psk.type = 'text';
                toggleIcon.innerText = '🙈';
            } else {
                psk.type = 'password';
                toggleIcon.innerText = '👁️';
            }
        }

        function toggleCustomSsid() {
            const select = document.getElementById('ssid_select');
            const customGroup = document.getElementById('ssid_custom_group');
            customGroup.style.display = select.value === 'custom' ? 'block' : 'none';
        }

        function showMessage(elementId, text, type) {
            const el = document.getElementById(elementId);
            el.innerText = text;
            el.style.display = 'block';
            el.className = `status-message ${type === 'success' ? 'status-success' : 'status-error'}`;
        }

        function fetchStatus() {
            fetch('/api/status')
                .then(res => res.json())
                .then(data => {
                    // Dynamic system status calculation
                    let statusText = "🟢 Opérationnel";
                    let statusClass = "badge-green";
                    if (data.network_mode === 'AccessPoint') {
                        statusText = "🟡 Avertissement (Mode AP)";
                        statusClass = "badge-warn";
                    } else if (data.ip_addr === '0.0.0.0' || !data.wifi_ssid || data.wifi_ssid === 'Non connecté') {
                        statusText = "🔴 Erreur (Déconnecté)";
                        statusClass = "badge-error";
                    }
                    const statusBadge = document.getElementById('system_status_badge');
                    if (statusBadge) {
                        statusBadge.innerText = statusText;
                        statusBadge.className = `badge ${statusClass}`;
                    }

                    document.getElementById('network_mode').innerText = data.network_mode;
                    document.getElementById('network_mode').className = `badge ${data.network_mode === 'Station' ? 'badge-green' : 'badge-blue'}`;
                    document.getElementById('wifi_ssid').innerText = data.wifi_ssid || 'Non connecté';
                    document.getElementById('wifi_rssi').innerText = data.wifi_rssi ? `${data.wifi_rssi} dBm` : '- dBm';
                    document.getElementById('ip_addr').innerText = data.ip_addr || '0.0.0.0';
                    document.getElementById('gateway_addr').innerText = data.gateway_addr || '0.0.0.0';
                    document.getElementById('sys_time').innerText = data.sys_time || '1970-01-01 00:00:00';
                    document.getElementById('ntp_server').innerText = data.ntp_server || 'Non configuré';
                    document.getElementById('prod_version').innerText = data.fw_version || 'v1.0.0-poc';
                    document.getElementById('last_ota').innerText = data.last_ota_success || 'Jamais';
                    
                    if (document.getElementById('update_url') && data.update_url) {
                        document.getElementById('update_url').value = data.update_url;
                    }
                })
                .catch(err => console.error('Erreur status:', err));
        }

        function fetchSensors() {
            if (!document.getElementById('sensors-tab').classList.contains('active')) return;
            
            fetch('/api/sensors')
                .then(res => res.json())
                .then(data => {
                    document.getElementById('val_temp_sht').innerText = data.temperature_sht45.toFixed(1);
                    document.getElementById('val_humi_sht').innerText = data.humidity_sht45.toFixed(1);
                    document.getElementById('val_co2').innerText = data.co2_scd41;
                    document.getElementById('val_temp_ds').innerText = data.temperature_ds18b20.toFixed(1);
                })
                .catch(err => console.error('Erreur sensors:', err));
        }

        function toggleRelay() {
            const check = document.getElementById('relay_1_check');
            const intensity = parseInt(document.getElementById('pwm_slider').value);
            
            fetch('/api/actuators', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    relay_1: check.checked,
                    pwm_intensity: intensity
                })
            })
            .catch(err => console.error('Erreur relay switch:', err));
        }

        function updatePwm(val) {
            const check = document.getElementById('relay_1_check');
            
            fetch('/api/actuators', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    relay_1: check.checked,
                    pwm_intensity: parseInt(val)
                })
            })
            .catch(err => console.error('Erreur pwm slider:', err));
        }

        function scanSsids() {
            const select = document.getElementById('ssid_select');
            fetch('/api/ssids')
                .then(res => res.json())
                .then(ssids => {
                    const prevVal = select.value;
                    select.innerHTML = '<option value="custom">-- Saisir un SSID --</option>';
                    ssids.forEach(ssid => {
                        const opt = document.createElement('option');
                        opt.value = ssid;
                        opt.innerText = ssid;
                        select.appendChild(opt);
                    });
                    if (prevVal && prevVal !== 'custom') {
                        select.value = prevVal;
                        toggleCustomSsid();
                    }
                })
                .catch(err => console.error('Erreur scan:', err));
        }

        function saveConfiguration() {
            const select = document.getElementById('ssid_select');
            let ssid = select.value;
            if (ssid === 'custom') {
                ssid = document.getElementById('ssid_custom').value;
            }
            const password = document.getElementById('password').value;
            const updateUrl = document.getElementById('update_url').value;

            if (!ssid) {
                showMessage('config_status', 'Le SSID est requis !', 'error');
                return;
            }

            showMessage('config_status', 'Enregistrement en cours. L\'ESP32 va redémarrer immédiatement...', 'success');

            fetch('/api/config', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({
                    wifi_ssid: ssid,
                    wifi_psk: password,
                    update_url: updateUrl
                })
            })
            .then(res => {
                if (res.ok) {
                    showMessage('config_status', 'Sauvegarde réussie ! Redémarrage...', 'success');
                } else {
                    showMessage('config_status', 'Erreur lors de la sauvegarde.', 'error');
                }
            })
            .catch(err => showMessage('config_status', 'Erreur de connexion : ' + err, 'error'));
        }

        // Periodic sensor readings
        setInterval(fetchSensors, 2000);
        setInterval(fetchStatus, 6000);
        
        // Initial setup
        fetchSensors();
    </script>
</body>
</html>
"#;

pub const FACTORY_HTML: &str = r#"<!DOCTYPE html>
<html lang="fr">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>WhisperEye - Secours & Provisionnement</title>
    <style>
        :root {
            --bg-color: #0f172a;
            --card-bg: #1e293b;
            --accent-color: #06b6d4;
            --accent-hover: #0891b2;
            --text-primary: #f8fafc;
            --text-secondary: #94a3b8;
            --border-color: #334155;
            --success: #10b981;
            --error: #ef4444;
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
            max-width: 600px;
            width: 100%;
        }

        h1 {
            font-size: 2.2rem;
            font-weight: 800;
            background: linear-gradient(135deg, #06b6d4 0%, #3b82f6 100%);
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
            max-width: 600px;
            background-color: var(--card-bg);
            border: 1px solid var(--border-color);
            border-radius: 16px;
            box-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.3), 0 8px 10px -6px rgba(0, 0, 0, 0.3);
            overflow: hidden;
        }

        .tabs {
            display: flex;
            background-color: rgba(15, 23, 42, 0.5);
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
            background-color: rgba(255, 255, 255, 0.05);
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
            border-bottom: 1px solid rgba(51, 65, 85, 0.5);
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

        .badge-cyan {
            background-color: rgba(6, 182, 212, 0.2);
            color: var(--accent-color);
        }

        .badge-success {
            background-color: rgba(16, 185, 129, 0.2);
            color: var(--success);
        }

        .badge-error {
            background-color: rgba(239, 68, 68, 0.2);
            color: var(--error);
        }

        .badge-warn {
            background-color: rgba(245, 158, 11, 0.2);
            color: #f59e0b;
        }

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
            background-color: rgba(15, 23, 42, 0.6);
            border: 1px solid var(--border-color);
            border-radius: 8px;
            color: var(--text-primary);
            font-size: 1rem;
            outline: none;
            transition: border-color 0.2s ease;
        }

        input:focus, select:focus {
            border-color: var(--accent-color);
        }

        .checkbox-group {
            display: flex;
            align-items: center;
            gap: 0.5rem;
            margin-top: 0.5rem;
        }

        .checkbox-group input {
            width: auto;
            cursor: pointer;
        }

        .btn {
            width: 100%;
            padding: 0.85rem 1.5rem;
            background-color: var(--accent-color);
            color: #000;
            font-weight: 700;
            font-size: 1rem;
            border: none;
            border-radius: 8px;
            cursor: pointer;
            transition: all 0.2s ease;
            display: flex;
            justify-content: center;
            align-items: center;
            gap: 0.5rem;
        }

        .btn:hover {
            background-color: var(--accent-hover);
            transform: translateY(-1px);
        }

        .btn:active {
            transform: translateY(0);
        }

        .file-upload-area {
            border: 2px dashed var(--border-color);
            border-radius: 12px;
            padding: 2.5rem 1.5rem;
            text-align: center;
            cursor: pointer;
            transition: border-color 0.2s ease;
            position: relative;
        }

        .file-upload-area:hover {
            border-color: var(--accent-color);
        }

        .file-upload-area input[type="file"] {
            position: absolute;
            top: 0;
            left: 0;
            width: 100%;
            height: 100%;
            opacity: 0;
            cursor: pointer;
        }

        .progress-container {
            margin-top: 1.5rem;
            display: none;
        }

        .progress-bar-bg {
            width: 100%;
            height: 8px;
            background-color: var(--border-color);
            border-radius: 9999px;
            overflow: hidden;
            margin-bottom: 0.5rem;
        }

        .progress-bar {
            height: 100%;
            width: 0%;
            background-color: var(--accent-color);
            border-radius: 9999px;
            transition: width 0.1s ease;
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
            border: 1px solid var(--success);
            color: var(--success);
            display: block;
        }

        .status-error {
            background-color: rgba(239, 68, 68, 0.15);
            border: 1px solid var(--error);
            color: var(--error);
            display: block;
        }

        .pulse {
            display: inline-block;
            width: 8px;
            height: 8px;
            border-radius: 50%;
            background-color: var(--accent-color);
            box-shadow: 0 0 0 0 rgba(6, 182, 212, 0.7);
            animation: pulse 1.5s infinite;
            margin-right: 0.5rem;
        }

        @keyframes pulse {
            0% {
                transform: scale(0.95);
                box-shadow: 0 0 0 0 rgba(6, 182, 212, 0.7);
            }
            70% {
                transform: scale(1);
                box-shadow: 0 0 0 6px rgba(6, 182, 212, 0);
            }
            100% {
                transform: scale(0.95);
                box-shadow: 0 0 0 0 rgba(6, 182, 212, 0);
            }
        }
    </style>
</head>
<body>
    <header>
        <h1>WhisperEye</h1>
        <p><span class="pulse"></span>Firmware Secours & Provisionnement (Factory)</p>
    </header>

    <div class="container">
        <div class="tabs">
            <button class="tab-btn active" onclick="switchTab('status-tab')">Statut ESP32</button>
            <button class="tab-btn" onclick="switchTab('config-tab')">Configuration</button>
            <button class="tab-btn" onclick="switchTab('upload-tab')">Flash Direct</button>
        </div>

        <!-- TAB 1: STATUS -->
        <div id="status-tab" class="tab-content active">
            <div class="info-group">
                <span class="info-label">État Système</span>
                <span class="info-value"><span id="system_status_badge" class="badge badge-success">🟢 Opérationnel</span></span>
            </div>
            <div class="info-group">
                <span class="info-label">Mode Réseau</span>
                <span class="info-value"><span id="network_mode" class="badge badge-cyan">Station</span></span>
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
                <span class="info-value" id="prod_version">empty</span>
            </div>
            <div class="info-group">
                <span class="info-label">Dernière OTA Success</span>
                <span class="info-value" id="last_ota">1970-01-01 00:00:00</span>
            </div>
        </div>

        <!-- TAB 2: CONFIGURATION -->
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
                <input type="text" id="update_url" value="https://github.com/sctfic/WhisperEye/raw/main/boards/board_default/firmware.bin">
            </div>

            <button class="btn" onclick="saveConfiguration()">Sauvegarder & Démarrer l'OTA</button>
            <div id="config_status" class="status-message"></div>
        </div>

        <!-- TAB 3: FLASH DIRECT -->
        <div id="upload-tab" class="tab-content">
            <div class="file-upload-area">
                <p style="font-size: 1.1rem; font-weight: 600; margin-bottom: 0.5rem; color: var(--text-primary);">
                    Glissez & Déposez le fichier de production .bin
                </p>
                <p style="color: var(--text-secondary); font-size: 0.85rem; margin-bottom: 1.5rem;">
                    ou cliquez pour parcourir vos fichiers
                </p>
                <input type="file" id="firmware_file" accept=".bin" onchange="handleFileSelected()">
                <div class="badge badge-cyan" id="selected_filename" style="display: none; width: fit-content; margin: 0 auto;"></div>
            </div>

            <div class="progress-container" id="progress_container">
                <div class="progress-bar-bg">
                    <div class="progress-bar" id="progress_bar"></div>
                </div>
                <div style="display: flex; justify-content: space-between; font-size: 0.85rem; color: var(--text-secondary);">
                    <span id="upload_percentage">0%</span>
                    <span id="upload_bytes">0 / 0 KB</span>
                </div>
            </div>

            <button class="btn" id="upload_btn" style="margin-top: 1.5rem; display: none;" onclick="startManualFlash()">
                Lancer le Flashage
            </button>
            <div id="upload_status" class="status-message"></div>
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

        function toggleCustomSsid() {
            const select = document.getElementById('ssid_select');
            const customGroup = document.getElementById('ssid_custom_group');
            if (select.value === 'custom') {
                customGroup.style.display = 'block';
            } else {
                customGroup.style.display = 'none';
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

        function handleFileSelected() {
            const fileInput = document.getElementById('firmware_file');
            const file = fileInput.files[0];
            if (file) {
                const label = document.getElementById('selected_filename');
                label.innerText = `${file.name} (${(file.size / 1024).toFixed(1)} KB)`;
                label.style.display = 'inline-block';
                document.getElementById('upload_btn').style.display = 'block';
            }
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
                    let statusClass = "badge-success";
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
                    document.getElementById('network_mode').className = `badge ${data.network_mode === 'Station' ? 'badge-success' : 'badge-cyan'}`;
                    document.getElementById('wifi_ssid').innerText = data.wifi_ssid || 'Non connecté';
                    document.getElementById('wifi_rssi').innerText = data.wifi_rssi ? `${data.wifi_rssi} dBm` : '- dBm';
                    document.getElementById('ip_addr').innerText = data.ip_addr || '0.0.0.0';
                    document.getElementById('gateway_addr').innerText = data.gateway_addr || '0.0.0.0';
                    document.getElementById('sys_time').innerText = data.sys_time || '1970-01-01 00:00:00';
                    document.getElementById('ntp_server').innerText = data.ntp_server || 'Non configuré';
                    document.getElementById('prod_version').innerText = data.fw_version || 'Aucun';
                    document.getElementById('last_ota').innerText = data.last_ota_success || 'Jamais';
                    
                    if (document.getElementById('update_url') && data.update_url) {
                        document.getElementById('update_url').value = data.update_url;
                    }
                })
                .catch(err => console.error('Erreur status:', err));
        }

        function scanSsids() {
            const select = document.getElementById('ssid_select');
            fetch('/api/ssids')
                .then(res => res.json())
                .then(ssids => {
                    // Save previous value
                    const prevVal = select.value;
                    
                    // Clear list but keep "custom"
                    select.innerHTML = '<option value="custom">-- Saisir un SSID --</option>';
                    
                    ssids.forEach(ssid => {
                        const opt = document.createElement('option');
                        opt.value = ssid;
                        opt.innerText = ssid;
                        select.appendChild(opt);
                    });

                    // Restore selection
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

            showMessage('config_status', 'Enregistrement en cours et démarrage de la procédure OTA...', 'success');

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
                    showMessage('config_status', 'Configuration enregistrée. L\'ESP32 tente de se connecter et lance la mise à jour OTA !', 'success');
                } else {
                    showMessage('config_status', 'Erreur lors de la sauvegarde.', 'error');
                }
            })
            .catch(err => showMessage('config_status', 'Erreur réseau : ' + err, 'error'));
        }

        function startManualFlash() {
            const fileInput = document.getElementById('firmware_file');
            const file = fileInput.files[0];
            if (!file) return;

            const progressContainer = document.getElementById('progress_container');
            const progressBar = document.getElementById('progress_bar');
            const percentageText = document.getElementById('upload_percentage');
            const bytesText = document.getElementById('upload_bytes');
            const uploadBtn = document.getElementById('upload_btn');
            
            progressContainer.style.display = 'block';
            uploadBtn.disabled = true;

            const xhr = new XMLHttpRequest();
            xhr.open('POST', '/api/upload-ota', true);
            xhr.setRequestHeader('Content-Type', 'application/octet-stream');

            xhr.upload.onprogress = function(e) {
                if (e.lengthComputable) {
                    const percentage = Math.round((e.loaded / e.total) * 100);
                    progressBar.style.width = `${percentage}%`;
                    percentageText.innerText = `${percentage}%`;
                    bytesText.innerText = `${(e.loaded / 1024).toFixed(1)} / ${(e.total / 1024).toFixed(1)} KB`;
                }
            };

            xhr.onload = function() {
                uploadBtn.disabled = false;
                if (xhr.status === 200) {
                    showMessage('upload_status', 'Flashage réussi ! Redémarrage sur le firmware de production en cours...', 'success');
                } else {
                    showMessage('upload_status', `Erreur de flashage (${xhr.status}) : ${xhr.responseText}`, 'error');
                }
            };

            xhr.onerror = function() {
                uploadBtn.disabled = false;
                showMessage('upload_status', 'Erreur réseau durant l\'upload.', 'error');
            };

            xhr.send(file);
        }

        // Initial fetch
        fetchStatus();
        setInterval(fetchStatus, 5000);
    </script>
</body>
</html>
"#;

1. Cycle d'Initialisation et de Connexion (

perform_wifi_connection
)
Au démarrage de l'application ou lors d'une demande de reconnexion, la fonction 

perform_wifi_connection
 effectue les opérations suivantes :

A. Lecture de la Configuration NVS
Le système charge les paramètres depuis la NVS :

meshEnabled : Activé par défaut (vaut 1 si absent).
wifiChannel : Canal Wi-Fi actif, par défaut 11.
meshId : SSID du point d'accès Mesh, par défaut "Whisper".
known_networks : Liste des réseaux Wi-Fi configurés.
default_net_psk : Clé de sécurité du réseau client Wi-Fi par défaut.
B. Détermination du mode "Open AP" (Sans sécurité)
Le point d'accès Mesh local démarre en mode ouvert (sans mot de passe, AuthMethod::None) si :

Le mode appairage est actif (pairing_until dans le futur).
OU le réseau client par défaut n'a pas de mot de passe (default_net_psk est vide).
Sinon, il utilise default_net_psk comme mot de passe WPA2 (AuthMethod::WPA2Personal).

2. Comportement avec Mesh ACTIVÉ (meshEnabled == 1)
Dans ce mode, l'appareil gère simultanément son rôle de point d'accès (AP Mesh) et de client (STA) :

Démarrage immédiat de l'AP Mesh (

start_mesh_ap_only
)

L'AP démarre instantanément en tâche de fond avec le meshId et le canal déterminé.
Si l'AP tourne déjà avec la bonne configuration, le système ignore le redémarrage pour éviter de déconnecter les clients connectés.
Un serveur DNS Captif (

run_captive_dns_server
) est démarré sur le port UDP 53 afin de rediriger toutes les requêtes des clients connectés vers 192.168.71.1.
Scan Réseau

L'appareil effectue un scan Wi-Fi matériel pour détecter les réseaux dans la zone et met à jour le cache wifi.scan_cache.
Tentative de connexion STA aux réseaux connus (

try_sta_on_mesh
)

Le système essaie de se connecter au réseau client Wi-Fi défini comme par défaut (si détecté lors du scan).
En cas d'échec ou d'absence, il tente la connexion aux autres réseaux connus de la NVS présents dans le scan.
Comportement de try_sta_on_mesh : Il reconfigure la radio en mode mixte (STA + AP) et tente d'obtenir un bail DHCP. Si la connexion STA échoue, il se déconnecte proprement et restaure le mode AP seule, garantissant ainsi que l'AP locale reste accessible sans coupure pour les utilisateurs.
En cas de succès, le nœud est désigné comme Root (is_root = true, distance = 0), la LED passe au Vert (luminosité 10%), et le canal réel de connexion est sauvegardé dans la NVS sous la clé wifiChannel.
Connexion Parent Mesh (si déconnecté du Wi-Fi local)

Si aucun réseau Wi-Fi local n'est accessible mais que le SSID du Mesh (meshId) est détecté dans le scan, le WhisperEye tente de s'y connecter en mode client (STA).
En cas de succès, il déclenche une synchronisation HTTP (

perform_mesh_sync
) auprès du parent à l'adresse http://192.168.71.1/api/mesh/sync.
Le parent lui renvoie les informations réseau (SSID client, clé PSK, serveur NTP, distance). Le nœud met à jour sa NVS et définit sa distance : distance = distance_parent + 1. Il est désigné comme Enfant (is_root = false).
AP Seule (Aucune connexion client)

Si aucune connexion STA (Wi-Fi classique ou Parent) n'aboutit, l'AP Mesh reste active. Le nœud a une distance = -1, is_root = false, et la LED RGB passe au Jaune (luminosité 10%).
3. Comportement avec Mesh DÉSACTIVÉ (meshEnabled == 0)
Dans ce mode classique, l'AP Mesh n'est pas démarrée au boot :

Le système essaie uniquement de se connecter en tant que station (STA) aux réseaux enregistrés (le réseau par défaut en premier, puis les autres).
En cas de succès, la LED passe au Vert et le canal Wi-Fi actif est sauvegardé en NVS.
En cas d'échec total :
Au démarrage (is_boot == true) : L'appareil démarre l'AP de secours "ESP32-Configuration" en mode ouvert sur le canal 6, avec le serveur DNS Captif. La LED passe au Jaune.
En fonctionnement : Il reste déconnecté en mode STA. La LED passe au Jaune.
4. Mode d'Appairage (Pairing Mode / Open AP)
Le mode d'appairage permet de rendre l'AP Mesh ouverte temporairement afin de synchroniser de nouveaux nœuds enfants ou de s'y connecter sans mot de passe. Il est piloté de deux manières :

A. Le bouton physique BOOT (GPIO0)
Le thread boot_button_worker vérifie l'état de la broche GPIO0 toutes les 100 ms.
Si le bouton BOOT reste enfoncé pendant 2 secondes, le mode appairage est activé pour 120 secondes (pairing_until est défini) et une reconnexion Wi-Fi est provoquée. L'AP Mesh redémarre alors sans mot de passe.
B. API HTTP /api/mesh/pair (POST)
Déclenche immédiatement le mode appairage pour 120 secondes et réapplique la configuration Wi-Fi ouverte.
C. Fermeture automatique & Prolongation
Lorsqu'un nœud enfant effectue sa synchronisation HTTP sur GET /api/mesh/sync auprès d'un parent en cours d'appairage, le parent réduit immédiatement la durée restante de l'appairage à 10 secondes (fermeture sécurisée rapide après intégration).
La macro extend_pairing! prolonge automatiquement l'appairage de 120 secondes supplémentaires à chaque fois que l'API d'état (ex: /api/status, /api/peripherals) est interrogée, permettant de maintenir l'AP ouverte tant que l'utilisateur navigue activement sur le dashboard d'administration.
5. API HTTP Spécifiques au Wi-Fi & Mesh
GET /api/status : Fournit le mode réseau actuel (Station, AccessPoint, Mixed, None), le SSID, la force du signal (RSSI), l'IP/masque/passerelle, ainsi que l'état détaillé du Mesh (actif/inactif, root/enfant, distance, nombre de nœuds enfants connectés, canal, et secondes d'appairage restantes).
GET /api/ssids : Scanne les réseaux à portée. En cas d'échec (ex: en AP pure où le scan matériel ESP32 échoue), l'API renvoie de façon transparente le cache de scan du boot ou une liste générique de secours.
GET /api/mesh/sync : Endpoint appelé par les enfants. Il insère l'adresse MAC de l'enfant dans la liste des nœuds connectés, réduit le timer d'appairage à 10 secondes, et renvoie la configuration réseau du Root.
POST /api/config :
Si wifi_ssid est modifié : Tente immédiatement de se connecter au nouveau réseau. En cas d'échec, le firmware applique une logique de reversion (revert) vers l'ancienne configuration Wi-Fi cliente fonctionnelle, ou vers d'autres réseaux connus. Si tout échoue, il redémarre l'AP.
Si les paramètres du Mesh (mesh_enabled, mesh_id, mesh_pmk) changent :
Si apply_only == true : Met à jour l'état en NVS et réapplique la configuration Wi-Fi à la volée en tâche de fond.
Si apply_only == false (comportement par défaut) : Enregistre les données et déclenche un redémarrage complet de l'ESP32 dans les 2 secondes pour appliquer proprement la structure du Mesh.
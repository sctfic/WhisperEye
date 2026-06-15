# Réimplémentation de la couche Wi-Fi et Mesh avec mode Bridge

Ce plan décrit l'approche technique pour réimplémenter la couche Wi-Fi et Mesh de WhisperEye afin d'améliorer la fiabilité, la robustesse de la synchronisation, et la joignabilité de tous les nœuds depuis le réseau de la box.

## Contraintes du client (à respecter)

- **NVS en clair** : Ne pas chiffrer les valeurs stockées dans la NVS. Toutes les variables (PSK, TOTP, etc.) doivent être sauvegardées en clair, telles qu'elles sont actuellement dans `common/src/nvs_storage.rs`.
- **Pas de tests unitaires** : Ne pas ajouter de suites de tests unitaires ou de scaffolding de test dans cette tâche.
- **Ne pas toucher l'OTA** : La logique OTA (détection, téléchargement et rollback) existante doit rester inchangée.


## Réponses aux commentaires de l'utilisateur

> [!IMPORTANT]
> **Changement de Root et Route Statique IP :**
> * **Comportement :** Si le nœud Root change (panne, perte de signal), le nouveau Root obtiendra une IP DHCP différente de la Box de par sa propre adresse MAC. **La route statique configurée sur la Box ne sera plus valide** car elle pointera vers l'ancienne IP.
> * **Recommandation :** C'est pourquoi nous recommandons l'utilisation du **Reverse Proxy HTTP** natif. Celui-ci s'adapte automatiquement sur le nœud Root actif. L'utilisateur ou le système domotique peut découvrir dynamiquement la nouvelle IP du Root (ou en utilisant la résolution de nom mDNS comme `whispereye.local` si active) et interroger `/proxy/<MAC_ADRESSE>/...` sans rien changer sur la box.
> * **Reste joignable comme nœud Mesh :** Lorsqu'un Root perd sa connexion Wi-Fi de la box et démissionne, il devient un nœud enfant (Mesh) standard, cherche à s'associer à un parent Mesh fonctionnel, et reste ainsi pleinement opérationnel et joignable dans le réseau local Mesh.

## User Review Required

> [!IMPORTANT]
> **Le Mode Bridge (L2 vs L3 NAPT) :**
> * **Limitation Wi-Fi L2 :** Un pont transparent de niveau 2 (Layer 2 Bridge) sur l'interface STA Wi-Fi de l'ESP32 n'est pas possible en raison de la limite des 3 adresses MAC imposée par les normes Wi-Fi standards (le routeur de la box rejette les paquets émis avec des MAC sources différentes du STA connecté).
> * **Solution - Mode Bridge IP (NAPT) :** Nous utilisons le mode pont IP de niveau 3 avec **NAPT** (Network Address Port Translation) d'ESP-IDF (`CONFIG_LWIP_IPV4_NAPT=y`). Tous les nœuds du maillage et clients AP accèdent à la box de manière transparente, masqués derrière l'IP STA de l'ESP32 Root.
> * **Joignabilité descendante (depuis la Box) :** Comme le NAPT est unidirectionnel (sortant), nous proposons deux méthodes pour accéder aux nœuds enfants depuis le réseau de la box :
>   1. **Reverse Proxy HTTP (inclus nativement) :** Le nœud Root expose un endpoint `/proxy/<MAC_ADRESSE>/<PATH>` (ex: `http://<IP_ROOT>/proxy/30:AE:A4:07:0B:0C/api/status`) qui relaie la requête HTTP en interne vers le nœud enfant. Cela permet d'accéder au frontend de n'importe quel nœud sans configuration réseau.
>   2. **Routage IP natif (Optionnel) :** Grâce à `CONFIG_LWIP_IP_FORWARD=y`, si l'utilisateur ajoute une route statique sur sa box pour rediriger le réseau du Mesh (`192.168.70.0/22`) vers l'adresse IP du Root, chaque nœud redevient pingable et joignable directement par son adresse IP unique (valable tant que le Root physique ne change pas).

> [!IMPORTANT]
> **Algorithme de Reconnexion et Démission (Resignation) :**
> * Si la connexion avec la box Wi-Fi est perdue, le Root tente de se reconnecter avec un **backoff exponentiel** aux intervalles suivants : **2s, 4s, 8s, 16s, et max 32s**.
> * Si l'échec persiste après 5 tentatives (~1 minute au total), le nœud "démissionne" de son rôle de Root pour chercher et rejoindre un parent Mesh existant afin de maintenir la cohésion locale du réseau (et reste joignable comme nœud Mesh).

## Open Questions

Aucune question ouverte. Le plan intègre vos retours (mode Bridge/NAPT, limite de retry max à 32s, comportement du nœud démissionnaire).

---

## Proposed Changes

### 1. Correction de la Compilation et Alignement

La compilation échoue actuellement car `main.rs` et `cron.rs` ont été partiellement modifiés mais `wifi.rs` contient toujours l'ancienne structure `WifiManager`. Nous allons unifier cela sous la structure `NetManager` et la machine d'état `NetState`.

#### [MODIFY] [wifi.rs](file:///c:/Users/Alban/Desktop/Dev/www/Probe-IoT/WhisperEye/production_app/src/wifi.rs)
* Remplacer `WifiManager` par `NetManager`.
* Définir la machine d'état `NetState` :
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum NetState {
      WifiPreferred, // Tente de se connecter au Wi-Fi de la box
      WifiOk,        // Connecté au Wi-Fi de la box (Root)
      WifiFallback,  // Perdu box, cherche un parent Mesh
      MeshOk,        // Connecté à un parent Mesh (Enfant)
      ApPairing,     // Mode appairage portail captif (120s)
  }
  ```
* Implémenter l'activation du NAPT sur l'interface SoftAP de l'ESP32 via la fonction FFI `esp_netif_napt_enable` ou `ip_napt_enable` d'lwIP.
* Configurer le serveur DHCP pour attribuer des sous-réseaux IP dynamiques sans chevauchement selon la distance réseau :
  * Root : `192.168.71.1` (Subnet `192.168.71.0/24`)
  * Enfant Niveau 1 : `192.168.72.1` (Subnet `192.168.72.0/24`)
  * Enfant Niveau 2 : `192.168.73.1` (Subnet `192.168.73.0/24`)
  * Mode Portail Captif (`ApPairing`) : `192.168.70.1` (Subnet `192.168.70.0/24`)
* Adapter les fonctions de scan actif (`active_scan_ssid`) et de tentatives de connexion (`try_sta_connect`).

#### [MODIFY] [cron.rs](file:///c:/Users/Alban/Desktop/Dev/www/Probe-IoT/WhisperEye/production_app/src/cron.rs)
* Mettre à jour les références de `WifiManager` vers `NetManager`.
* Déplacer la logique d'évaluation périodique du réseau dans un gestionnaire non bloquant. Le cron pilotera périodiquement les transitions de la machine d'état `NetState` (chaque tick) sans bloquer les mesures des capteurs.
* Gérer les transitions et la mise à jour des patterns de la LED de statut réseau :
  * `ApPairing` -> Heartbeat
  * `WifiPreferred` / `WifiFallback` -> Clignotement rapide
  * Connecté (`WifiOk` / `MeshOk`) -> SlowPulse (respiration douce)
  * Déconnecté / AP seule -> Clignotement lent / Off

#### [MODIFY] [main.rs](file:///c:/Users/Alban/Desktop/Dev/www/Probe-IoT/WhisperEye/production_app/src/main.rs)
* Mettre à jour le endpoint `/api/config` pour utiliser `wifi.try_sta_connect(ssid, &final_psk)` au lieu de `try_sta_on_mesh`.
* Implémenter le Reverse Proxy HTTP sous `/proxy/<MAC_ADRESSE>/<PATH>` sur le Root pour relayer les requêtes HTTP entrantes vers les nœuds enfants (en utilisant leur IP dynamique actuelle associée à leur MAC lors des synchronisations régulières `/api/mesh/sync`).

---

## Verification Plan

### Automated Tests
* Compiler le projet en mode release pour vérifier le respect de la taille limite (2 Mo) :
  ```powershell
  ./run.ps1 -Build -Package production_app
  ```

### Manual Verification
1. **Validation du Mode Bridge (NAPT) :**
   * Vérifier que les nœuds enfants peuvent pinguer la box principale et accéder à internet via le nœud Root.
   * Valider la joignabilité via le Reverse Proxy du Root : `http://<IP_ROOT>/proxy/<MAC_ENFANT>/api/status`.
2. **Algorithme de Reconnexion (Backoff) :**
   * Éteindre la box Wi-Fi. Mesurer les intervalles de tentative du Root (2s, 4s, 8s, 16s, max 32s).
   * Confirmer que le Root démissionne après environ 1 minute et repasse en Enfant s'il détecte un parent Mesh (et reste pleinement joignable).
3. **Bouton BOOT (2s) :**
   * Maintenir BOOT pendant 2s : vérifier le passage immédiat en AP ouvert sur l'IP `192.168.70.1`.
4. **Vérification de la taille :**
   * S'assurer que le binaire final produit par Cargo respecte la limite stricte de 2 Mo.

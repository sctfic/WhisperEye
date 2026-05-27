## Procedure de compilation et d'upload depuis antigravity IDE (avec si besoin Platform.IO)

Cette procédure décrit comment compiler et téléverser (uploader) le firmware de la carte **WhisperEye** (basée sur l'ESP32-S3) directement depuis l'IDE Antigravity ou via des outils complémentaires comme PlatformIO.

---

### 0. Méthode Ultra-Rapide (Recommandée) : Un Seul Script de Bout en Bout

Nous avons mis en place un script PowerShell utilitaire à la racine du projet nommé `we.ps1` (pour **W**hisper**E**ye). Il permet de charger l'environnement Xtensa, de configurer le dossier de cache temporaire, de compiler, de téléverser (uploader) et d'afficher les logs sur la console en **une seule commande ultra-courte** !

Depuis la racine du projet, ouvrez votre terminal PowerShell Antigravity et lancez :

```powershell
# En mode Debug (développement rapide)
.\we

# En mode Release (optimisé pour la taille et la vitesse)
.\we -Release

# Pour nettoyer le cache de build puis compiler et flasher
.\we -Clean

# Vous pouvez également lui passer des arguments supplémentaires (ex: spécifier le port COM et la vitesse/baudrate)
.\we -Release --port COM3 -b 115200
```

---

### 1. Méthode Native Rust avec Cargo (Manuelle)

C'est la méthode de référence pour WhisperEye. Elle utilise la chaîne de compilation Xtensa officielle d'Espressif pour Rust et l'utilitaire `espflash`.

#### A. Configuration de l'environnement Xtensa
Avant de compiler, vous devez charger les variables d'environnement nécessaires dans votre terminal PowerShell Antigravity :

```powershell
# 1. Charger la toolchain ESP
. C:\Users\Alban\export-esp.ps1

# 2. Définir le dossier de cache cible (très recommandé sous Windows pour accélérer les builds et éviter les chemins de fichiers trop longs)
$env:CARGO_TARGET_DIR = "C:\t"
```

#### B. Compilation du Firmware
Pour compiler la déclinaison par défaut de la carte (`board_default`) :

```powershell
# Naviguer dans le répertoire de la carte par défaut
cd boards/board_default

# Lancer la compilation avec la toolchain Rust 'esp'
cargo +esp build
```
*(Note : La première compilation peut prendre 4 à 8 minutes car elle télécharge et compile l'intégralité du framework ESP-IDF C++ en arrière-plan).*

#### C. Téléversement (Upload) & Moniteur Série
Pour flasher le firmware et observer les logs console en temps réel :

```powershell
# Téléverser le binaire sur l'ESP32-S3 connecté et démarrer le moniteur
cargo +esp espflash flash --monitor
```

---

### 2. Méthode avec PlatformIO (Optionnelle)

Si vous utilisez PlatformIO pour gérer vos téléversements ou vos cartes, vous pouvez l'utiliser de deux manières différentes.

#### A. Téléversement du binaire Rust pré-compilé via PlatformIO
PlatformIO embarque son propre outil `esptool.py`. Vous pouvez l'appeler depuis le terminal Antigravity IDE pour uploader le binaire `.bin` généré par Cargo :

1. Générez le binaire release avec Cargo :
   ```powershell
   cargo +esp build --release
   ```
2. Flashez le fichier `.bin` obtenu avec l'outil de PlatformIO :
   ```powershell
   # Remplacer 'COM3' par le port réel de votre ESP32-S3
   pio pkg exec -p tool-esptoolpy -- esptool.py --chip esp32s3 --port COM3 --baud 921600 write_flash 0x0 C:\t\xtensa-esp32s3-espidf\release\board_default.bin
   ```

#### B. Intégration en projet hybride (PlatformIO + Rust)
Si vous souhaitez que PlatformIO compile du code C++ et appelle votre bibliothèque Rust :

1. Déclarez un projet standard dans un fichier `platformio.ini` à la racine :
   ```ini
   [env:esp32s3]
   platform = espressif32
   board = esp32-s3-devkitc-1
   framework = espidf
   monitor_speed = 115200
   ```
2. Configurez Rust en tant que bibliothèque statique (`staticlib`) dans `Cargo.toml` :
   ```toml
   [lib]
   crate-type = ["staticlib"]
   ```
3. Liez la bibliothèque compilée `C:\t\xtensa-esp32s3-espidf\release\libcommon.a` dans vos scripts de build PlatformIO pour l'appeler via C FFI depuis votre code C++ standard.

---

### 3. Astuces & Dépannage

* **Port COM non détecté :** Vérifiez la bonne installation des pilotes USB-to-UART (CP210x ou CH34x) sous Windows.
* **Erreur `Cannot locate argument --ldproxy-linker` :** Cette erreur se produit lorsque le cache du build script `esp-idf-sys` est désaligné. Nettoyez le cache et recommencez :
  ```powershell
  cargo +esp clean
  cargo +esp build
  ```
* **Bootloader manuel :** Si l'upload automatique échoue :
  1. Maintenez le bouton **BOOT** (ou GPIO 0) appuyé.
  2. Appuyez brièvement sur le bouton **EN/RST**.
  3. Relâchez le bouton **BOOT**.
  4. Lancez la commande d'upload.

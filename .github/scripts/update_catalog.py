import os
import re
import json

def update_chip_catalog(chip_id, chip_name, json_filename, prefix):
    boards_dir = "boards/board_default"
    json_path = os.path.join(boards_dir, json_filename)

    if not os.path.exists(boards_dir):
        print(f"Warning: {boards_dir} not found.")
        return

    # Scanner les fichiers binaires pour ce préfixe
    files = [f for f in os.listdir(boards_dir) if f.startswith(prefix) and f.endswith(".bin")]

    stable_entries = []
    unstable_entries = []

    # Regex patterns
    unstable_pat = re.compile(rf"^{prefix}(\d+)\.(\d+)\.(\d+)-(\d+)\.bin$")
    stable_pat = re.compile(rf"^{prefix}(\d+)\.(\d+)\.(\d+)\.bin$")

    for filename in files:
        url = f"https://github.com/sctfic/WhisperEye/raw/main/boards/board_default/{filename}"
        
        m_unstable = unstable_pat.match(filename)
        if m_unstable:
            maj, min_val, pat, bld = map(int, m_unstable.groups())
            sort_key = maj * 100000000 + min_val * 1000000 + pat * 100000 + bld
            unstable_entries.append({
                "version": f"{maj}.{min_val}.{pat}-{bld:04d}",
                "url": url,
                "sortKey": sort_key,
                "filename": filename
            })
            continue

        m_stable = stable_pat.match(filename)
        if m_stable:
            maj, min_val, pat = map(int, m_stable.groups())
            sort_key = maj * 100000000 + min_val * 1000000 + pat * 100000
            stable_entries.append({
                "version": f"{maj}.{min_val}.{pat}",
                "url": url,
                "sortKey": sort_key,
                "filename": filename
            })

    # Trier par sortKey décroissant
    stable_entries.sort(key=lambda x: x["sortKey"], reverse=True)
    unstable_entries.sort(key=lambda x: x["sortKey"], reverse=True)

    # Stable (garder 1 seule active et jusqu'à 3 précédentes stables)
    sorted_stable = stable_entries[:1]
    sorted_previous_stable = stable_entries[1:4]

    # Unstable (garder les 2 plus récentes)
    sorted_unstable = unstable_entries[:2]

    # Supprimer physiquement les binaires instables non conservés
    kept_unstable_filenames = {item["filename"] for item in sorted_unstable}
    for entry in unstable_entries:
        if entry["filename"] not in kept_unstable_filenames:
            file_path = os.path.join(boards_dir, entry["filename"])
            if os.path.exists(file_path):
                print(f"Removing old unstable binary: {file_path}")
                os.remove(file_path)

    # Construire le dictionnaire final
    catalog = {
        "ChipType": chip_name,
        "stable": {
            "version": sorted_stable[0]["version"],
            "url": sorted_stable[0]["url"]
        } if sorted_stable else None,
        "previous_stable": [
            {"version": item["version"], "url": item["url"]}
            for item in sorted_previous_stable
        ],
        "unstable": [
            {"version": item["version"], "url": item["url"]}
            for item in sorted_unstable
        ]
    }

    # Réécrire le JSON
    with open(json_path, 'w', encoding='utf-8') as f:
        json.dump(catalog, f, indent=4, ensure_ascii=False)
    
    print(f"Catalog {json_filename} successfully updated!")

def main():
    # ESP32-S3
    update_chip_catalog("s3", "ESP32-S3", "firmware-s3.json", "firmware-s3-")
    # ESP32-C6
    update_chip_catalog("c6", "ESP32-C6", "firmware-c6.json", "firmware-c6-")

if __name__ == "__main__":
    main()

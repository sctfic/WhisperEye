use esp_idf_hal::gpio::PinDriver;
use embedded_graphics::{
    prelude::*,
    mono_font::{ascii::{FONT_10X20, FONT_6X10}, MonoTextStyleBuilder},
    pixelcolor::Rgb565,
    text::Text,
    primitives::{Rectangle, Line, PrimitiveStyle},
};
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;
use crate::screen::{Screen, ST7789Display};
use crate::board::Board;
use crate::actuators::{Actuators, ActuatorsState};

// ── 1. FONCTIONS DE DESSIN POUR LES ICÔNES DE STATUT ──

// ─── Icône WiFi ──────────────────────────────────────────────────────────────
// Taille : 16 x 12 px (4 barres croissantes, contour noir)
//
// Barres (gauche→droite) :
//   Barre 0 : col 0-1, hauteur 3  → rangées 9-11
//   Barre 1 : col 4-5, hauteur 5  → rangées 7-11
//   Barre 2 : col 8-9, hauteur 7  → rangées 5-11
//   Barre 3 : col 12-13, hauteur 9 → rangées 3-11
//
//  Col : 0000 1111 2222 3333
//  Row : 0-1-2-3-4-5-6-7-8-9-10-11
//
/// Dessine une icône WiFi avec 4 barres de signal croissantes et contour noir
fn draw_wifi_icon<D>(display: &mut D, start_point: Point, connected: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = if connected { Rgb565::GREEN } else { Rgb565::RED };
    let border_color = Rgb565::BLACK;
    let base_y = start_point.y + 11; // Base de la barre de signal

    // 4 barres : hauteurs 3, 5, 7, 9 px, largeurs 2px, séparées de 2px
    for i in 0..4 {
        let x = start_point.x + (i * 4);
        let height = 3 + (i * 2);
        let y = base_y - height;

        // Contour noir (+1px autour)
        let _ = Rectangle::new(Point::new(x - 1, y - 1), Size::new(4, (height + 2) as u32))
            .into_styled(PrimitiveStyle::with_fill(border_color))
            .draw(display);

        // Barre de couleur
        let _ = Rectangle::new(Point::new(x, y), Size::new(2, height as u32))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(display);
    }
    Ok(())
}

// ─── Icône Bluetooth ─────────────────────────────────────────────────────────
// Taille : 7 x 13 px  (bitmap en binaire, MSB = colonne gauche)
//
// Colonne : 0123456
// Bit 6 = col 0 (gauche), Bit 0 = col 6 (droite)
//
// Row  0 : 0001000  = 0x08
// Row  1 : 0001100  = 0x0C
// Row  2 : 0001010  = 0x0A
// Row  3 : 1001001  = 0x49
// Row  4 : 0101010  = 0x2A
// Row  5 : 0011100  = 0x1C
// Row  6 : 0001000  = 0x08
// Row  7 : 0011100  = 0x1C
// Row  8 : 0101010  = 0x2A
// Row  9 : 1001001  = 0x49
// Row 10 : 0001010  = 0x0A
// Row 11 : 0001100  = 0x0C
// Row 12 : 0001000  = 0x08

const BT_ROWS: usize = 13;
const BT_COLS: usize = 7;
const BT_BITMAP: [u8; BT_ROWS] = [
    0b0001000, // Row 0
    0b0001100, // Row 1
    0b0001010, // Row 2
    0b1001001, // Row 3
    0b0101010, // Row 4
    0b0011100, // Row 5
    0b0001000, // Row 6
    0b0011100, // Row 7
    0b0101010, // Row 8
    0b1001001, // Row 9
    0b0001010, // Row 10
    0b0001100, // Row 11
    0b0001000, // Row 12
];

fn draw_bluetooth_icon<D>(display: &mut D, start_point: Point, active: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = if active { Rgb565::GREEN } else { Rgb565::new(15, 30, 15) };
    let x = start_point.x;
    let y = start_point.y;

    for row in 0..BT_ROWS {
        for col in 0..BT_COLS {
            let bit = (BT_BITMAP[row] >> (BT_COLS - 1 - col)) & 1;
            if bit == 1 {
                let _ = embedded_graphics::Pixel(
                    Point::new(x + col as i32, y + row as i32),
                    color,
                ).draw(display);
            }
        }
    }
    Ok(())
}

// ─── Icône I2C ───────────────────────────────────────────────────────────────
// Taille : 14 x 14 px  (bitmap en binaire, 14 bits utiles par ligne)

const I2C_ROWS: usize = 14;
const I2C_COLS: usize = 14;
const I2C_BITMAP: [u16; I2C_ROWS] = [
    // Reconstitué depuis les pixels originaux (col 0..13 de gauche à droite)
    0b11111111111111, // Row 0
    0b10000000000001, // Row 1
    0b00001110000000, // Row 2
    0b11100010111111, // Row 3
    0b10101110100001, // Row 4
    0b10101000101111, // Row 5
    0b10101110101000, // Row 6
    0b10100000101000, // Row 7
    0b10100000101000, // Row 8
    0b10100000101111, // Row 9
    0b10100000100001, // Row 10
    0b11100000111111, // Row 11
    0b00000000000000, // Row 12
    0b11111111111111, // Row 13
];

fn draw_i2c_icon<D>(display: &mut D, start_point: Point, active: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = if active { Rgb565::GREEN } else { Rgb565::new(15, 30, 15) };
    let x = start_point.x;
    let y = start_point.y;

    for row in 0..I2C_ROWS {
        for col in 0..I2C_COLS {
            let bit = (I2C_BITMAP[row] >> (I2C_COLS - 1 - col)) & 1;
            if bit == 1 {
                let _ = embedded_graphics::Pixel(
                    Point::new(x + col as i32, y + row as i32),
                    color,
                ).draw(display);
            }
        }
    }
    Ok(())
}

// ─── Icône 1-Wire ─────────────────────────────────────────────────────────────
// Taille : 14 x 7 px  (symbole « 1W » stylisé)

const OW_ROWS: usize = 7;
const OW_COLS: usize = 14;
const OW_BITMAP: [u16; OW_ROWS] = [
    // Reconstitué depuis les pixels originaux (col 0..12 de gauche à droite)
    0b011001000000010, // Row 0  (cols 0,1,4,12)
    0b111001000000010, // Row 1  (cols 0,1,4,12)
    0b011000100100100, // Row 2  (cols 0,1,5,8,11)
    0b011000010101000, // Row 3  (cols 0,1,6,8,10)
    0b111111101010111, // Row 4  (cols 0,1,2,3,4,6,8,10,11)
    0b000000000000000, // Row 5  (vide)
    0b111111111111111, // Row 6  (cols 0..12)
];

fn draw_onewire_icon<D>(display: &mut D, start_point: Point, active: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = if active { Rgb565::GREEN } else { Rgb565::new(15, 30, 15) };
    let x = start_point.x;
    let y = start_point.y;

    for row in 0..OW_ROWS {
        for col in 0..OW_COLS {
            let bit = (OW_BITMAP[row] >> (OW_COLS - 1 - col)) & 1;
            if bit == 1 {
                let _ = embedded_graphics::Pixel(
                    Point::new(x + col as i32, y + row as i32),
                    color,
                ).draw(display);
            }
        }
    }
    Ok(())
}

// ─── Icône Éclair ─────────────────────────────────────────────────────────────
// Taille : 7 x 15 px


const LIGHTNING_ROWS: usize = 15;
const LIGHTNING_COLS: usize = 7;
const LIGHTNING_BITMAP: [u8; LIGHTNING_ROWS] = [
    0b0000011, // Row 0
    0b0000101, // Row 1
    0b0001010, // Row 2
    0b0001010, // Row 3
    0b0010010, // Row 4
    0b0100100, // Row 5
    0b0101111, // Row 6
    0b1000001, // Row 7
    0b1111010, // Row 8
    0b0010010, // Row 9
    0b0100100, // Row 10
    0b0101000, // Row 11
    0b0101000, // Row 12
    0b1010000, // Row 13
    0b1100000, // Row 14
];

/// Dessine un éclair (vert si tension externe, jaune si USB)
fn draw_lightning_icon<D>(display: &mut D, start_point: Point, ext_power: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = if ext_power { Rgb565::GREEN } else { Rgb565::new(31, 63, 0) };
    let x = start_point.x;
    let y = start_point.y;

    for row in 0..LIGHTNING_ROWS {
        for col in 0..LIGHTNING_COLS {
            let bit = (LIGHTNING_BITMAP[row] >> (LIGHTNING_COLS - 1 - col)) & 1;
            if bit == 1 {
                let _ = embedded_graphics::Pixel(
                    Point::new(x + col as i32, y + row as i32),
                    color,
                ).draw(display);
            }
        }
    }
    Ok(())
}

// ── 2. INITIALISATION ET BOUCLE PRINCIPALE IHM ──

pub fn run_ihm(
    mut screen: Screen,
    mut display: ST7789Display,
    board: Arc<Mutex<Board>>,
    actuators: Arc<Mutex<Actuators>>,
    actuators_state: Arc<Mutex<ActuatorsState>>,
) -> Result<(), anyhow::Error> {
    log::info!("Starting Screen IHM thread (screen_display)...");

    let mut last_rendered_brightness = -1;
    let mut last_rendered_ina = false;
    let mut last_rendered_inb = false;
    let mut last_rendered_rla = false;
    let mut last_rendered_rlb = false;
    let mut last_touch_state = false;
    let mut last_wifi_connected = false;
    let mut last_i2c_count = 99u8;
    let mut last_onewire_count = 99u8;
    let mut last_rendered_ver = 9999u32;
    let mut last_rendered_ssid = String::new();
    let mut last_rendered_ip = String::new();
    let mut raw_update_ticks = 0;
    let mut sensor_refresh_ticks: u32 = 0;

    let mut last_wifi_icon_state: Option<bool> = None;
    let mut last_lightning_icon_state: Option<bool> = None;
    let mut last_bt_icon_state: Option<bool> = None;
    let mut last_ntp_state: Option<bool> = None;
    let mut last_time_str = String::new();

    let mut vsense_volts_val = 0.0f32;
    let mut isense_amps_val = 0.0f32;

    display.clear(Rgb565::BLACK)
        .map_err(|e| anyhow::anyhow!("Clear display error: {:?}", e))?;

    // Ligne séparatrice sous la barre de statut (Y=16)
    let _ = Rectangle::new(Point::new(0, 16), Size::new(320, 1))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::new(10, 20, 10)))
        .draw(&mut display);

    // Ligne séparatrice au-dessus de la barre du bas (Y=221)
    let _ = Rectangle::new(Point::new(0, 221), Size::new(320, 1))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::new(10, 20, 10)))
        .draw(&mut display);

    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(Rgb565::WHITE)
        .background_color(Rgb565::BLACK)
        .build();

    let green_style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(Rgb565::GREEN)
        .background_color(Rgb565::BLACK)
        .build();

    let red_style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(Rgb565::RED)
        .background_color(Rgb565::BLACK)
        .build();

    // Styles spécifiques aux barres de statut (petite police 6x10)
    let status_style_white = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(Rgb565::WHITE)
        .background_color(Rgb565::BLACK)
        .build();

    let status_style_green = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(Rgb565::GREEN)
        .background_color(Rgb565::BLACK)
        .build();

    let status_style_red = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(Rgb565::RED)
        .background_color(Rgb565::BLACK)
        .build();

    let status_style_gray = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(Rgb565::new(10, 20, 10)) // Gris moyen
        .background_color(Rgb565::BLACK)
        .build();

    let touch_style_active = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(Rgb565::WHITE)
        .background_color(Rgb565::new(0, 0, 31)) // Fond bleu, texte blanc
        .build();

    // Style barre du bas : texte noir sur fond blanc
    let taskbar_title_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(Rgb565::BLACK)
        .background_color(Rgb565::WHITE)
        .build();

    let taskbar_ver_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(Rgb565::new(10, 20, 10)) // gris sur fond noir
        .background_color(Rgb565::BLACK)
        .build();

    let mut last_btn2 = true;
    let mut last_btn3 = true;
    let mut btn3_pressed_ticks = 0u32;
    let mut last_brightness = 20i32;

    // ── Dessiner la barre du bas une fois (statique) ──
    // Fond blanc pour la ligne du haut (Y=223..232)
    let _ = Rectangle::new(Point::new(128, 218), Size::new(64, 10))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
        .draw(&mut display);
    // Fond noir pour la ligne du bas (Y=233..240)
    // let _ = Rectangle::new(Point::new(0, 232), Size::new(320, 10))
    //     .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
    //     .draw(&mut display);

    // Centrer "WhisperEye" (10 chars × 6px = 60px) → X = (320-60)/2 = 130, Y=231 (baseline FONT_6X10)
    let _ = Text::new("WhisperEye", Point::new(130, 220), taskbar_title_style)
        .draw(&mut display);

    // Numéro de version (police 6x10) centré dessous, Y=241 (baseline)
    let ver_str = format!("v{}", crate::FW_VERSION);
    let ver_w = ver_str.len() as i32 * 6;
    let ver_x = (320 - ver_w) / 2;
    let _ = Text::new(&ver_str, Point::new(ver_x, 241), taskbar_ver_style)
        .draw(&mut display);

    loop {
        let btn2_val = screen.btn2_driver.is_high();
        let btn3_val = screen.btn3_driver.is_high();

        // 0. Appliquer la luminosité si elle change
        let current_brightness = screen.read_brightness();
        if current_brightness != last_brightness {
            let _ = screen.set_backlight(current_brightness as u32);
            {
                let mut acts = actuators.lock().unwrap();
                let _ = acts.ina.set_speed(current_brightness);
                let _ = acts.inb.set_speed(current_brightness);
            }
            last_brightness = current_brightness;
        }

        // 1. Appui sur BTN2 réinitialise la luminosité à 20%
        if last_btn2 && !btn2_val {
            screen.brightness.store(20, Ordering::Relaxed);
        }
        last_btn2 = btn2_val;

        // 2. Chaque appui sur BTN3 changera l'état de INA/INB
        if last_btn3 && !btn3_val {
            let (next_ina, next_inb) = {
                let act = actuators_state.lock().unwrap();
                match (act.ina, act.inb) {
                    (false, false) => (true, false),
                    (true, false) => (false, true),
                    (false, true) => (true, true),
                    (true, true) => (false, false),
                }
            };

            // Appliquer au hardware
            {
                let mut acts = actuators.lock().unwrap();
                let _ = acts.write("ina", next_ina);
                let _ = acts.write("inb", next_inb);
            }

            // Mettre à jour l'état partagé
            {
                let mut act = actuators_state.lock().unwrap();
                act.ina = next_ina;
                act.inb = next_inb;
            }

            log::info!(
                "Etat change par BTN3 : INA = {}, INB = {}",
                if next_ina { 1 } else { 0 },
                if next_inb { 1 } else { 0 }
            );
        }

        // 2.bis Appui long sur BTN3 (2 secondes = 200 ticks de 10ms) coupe SWPWR
        if !btn3_val {
            btn3_pressed_ticks += 1;
            if btn3_pressed_ticks == 200 {
                let mut acts = actuators.lock().unwrap();
                let _ = acts.write("swpwr", false);
                let mut act = actuators_state.lock().unwrap();
                act.swpwr = false;
            }
        } else {
            btn3_pressed_ticks = 0;
        }
        last_btn3 = btn3_val;

        // Lecture des valeurs analogiques et de touch de Board
        raw_update_ticks += 1;
        sensor_refresh_ticks += 1;
        let should_update_raw = raw_update_ticks >= 10;
        let should_refresh_sensors = sensor_refresh_ticks >= 500; // ~5s à 10ms/tick
        if should_refresh_sensors {
            sensor_refresh_ticks = 0;
        }
        
        let readings = {
            let mut b = board.lock().unwrap();
            let (ina_act, inb_act) = {
                let act = actuators_state.lock().unwrap();
                (act.ina, act.inb)
            };
            b.read_value(ina_act, inb_act)
        };

        if should_update_raw {
            raw_update_ticks = 0;
            vsense_volts_val = readings.vsense_volts.unwrap_or(0.0);
            isense_amps_val = readings.isense_amps.unwrap_or(0.0);
        }

        // 3. Rallumer SWPWR si touch appuyé
        let current_touch = readings.touch;
        if current_touch && !last_touch_state {
            let mut acts = actuators.lock().unwrap();
            let _ = acts.write("swpwr", true);
            let mut act = actuators_state.lock().unwrap();
            act.swpwr = true;
        }

        // Récupérer l'état des actionneurs
        let (current_rla, current_rlb, current_ina, current_inb) = {
            let act = actuators_state.lock().unwrap();
            (act.rla, act.rlb, act.ina, act.inb)
        };
        
        let current_ver = crate::i2c::i2c_bme280::BME280_FOUND.load(Ordering::Relaxed) as u32;
        let current_i2c = crate::i2c::I2C_DEVICES_COUNT.load(Ordering::Relaxed);
        let current_onewire = crate::one_wire::ONEWIRE_DEVICES_COUNT.load(Ordering::Relaxed);
        let current_wifi = crate::wifi::WIFI_CONNECTED.load(Ordering::Relaxed);

        let (current_ssid, current_ip) = {
            let ssid = crate::wifi::CURRENT_SSID.lock().unwrap().clone();
            let ip = crate::wifi::CURRENT_IP.lock().unwrap().clone();
            (ssid, ip)
        };

        // ── NTP : synchronisé si l'heure système est post-2020 ──
        let ntp_synced = {
            use std::time::{SystemTime, UNIX_EPOCH};
            // 2020-01-01 = 1577836800 secondes
            const YEAR_2020: u64 = 1_577_836_800;
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() > YEAR_2020)
                .unwrap_or(false)
        };

        // ── Heure courante hh:mm ──
        let current_time_str = {
            use std::time::{SystemTime, UNIX_EPOCH};
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let total_mins = secs / 60;
            let hh = (total_mins / 60) % 24;
            let mm = total_mins % 60;
            format!("{:02}:{:02}", hh, mm)
        };

        // Déterminer les zones à redessiner
        let time_changed = current_time_str != last_time_str
            || Some(ntp_synced) != last_ntp_state;

        let status_changed = current_i2c != last_i2c_count
            || current_onewire != last_onewire_count
            || current_wifi != last_wifi_connected
            || current_touch != last_touch_state
            || should_update_raw
            || time_changed;

        let bottom_changed = current_ina != last_rendered_ina
            || current_inb != last_rendered_inb
            || current_rla != last_rendered_rla
            || current_rlb != last_rendered_rlb
            || current_ssid != last_rendered_ssid
            || current_ip != last_rendered_ip
            || should_update_raw;

        let middle_changed = current_brightness != last_rendered_brightness
            || current_touch != last_touch_state
            || current_ver != last_rendered_ver
            || should_refresh_sensors;

        // ── 3. RENDU DE LA BARRE DE STATUT (HAUT : Y=0..15) ──
        //
        // Disposition (X) :
        //   0  → Heure "hh:mm" (5 chars × 6px = 30px) → X=1..30
        //   31 → "NTP" 3 chars × 6px = 18px → X=31..48  (vert/rouge)
        //   52 → Icône I2C 14px → X=52..65
        //   67 → Nb I2C (2 chars) → X=66..77
        //   80 → Icône 1-Wire 9px → X=80..88
        //   90 → Nb 1-Wire (2 chars) → X=89..100
        //
        //   Droite (positions à partir de X=220) :
        //   220 → BT  7px → X=220..226
        //   231 → Wifi 16px → X=231..246
        //   251 → Éclair 7px → X=251..257
        //   261 → "USB" ou tension → X=261..
        //
        if status_changed {
            last_i2c_count = current_i2c;
            last_onewire_count = current_onewire;
            last_wifi_connected = current_wifi;
            last_touch_state = current_touch;

            // ── A. Heure et NTP ──
            if time_changed {
                last_time_str = current_time_str.clone();
                last_ntp_state = Some(ntp_synced);

                // Heure en blanc
                let _ = Text::new(&current_time_str, Point::new(1, 11), status_style_white)
                    .draw(&mut display);

                // "NTP" en vert si synchro, rouge sinon
                let ntp_style = if ntp_synced { status_style_green } else { status_style_red };
                let _ = Text::new("NTP", Point::new(32, 11), ntp_style)
                    .draw(&mut display);
            }

            // ── B. Icône I2C + compteur ──
            let _ = draw_i2c_icon(&mut display, Point::new(56, 1), current_i2c > 0);
            let i2c_str = format!("{:>2}", current_i2c);
            let _ = Text::new(&i2c_str, Point::new(70, 11), status_style_white)
                .draw(&mut display);

            // ── C. Icône 1-Wire + compteur ──
            let _ = draw_onewire_icon(&mut display, Point::new(90, 1), current_onewire > 0);
            let ow_str = format!("{:>2}", current_onewire);
            let _ = Text::new(&ow_str, Point::new(102, 11), status_style_white)
                .draw(&mut display);

            // ── D. Bluetooth (fixe, inactif) ──
            if last_bt_icon_state.is_none() {
                last_bt_icon_state = Some(false);
                let _ = Rectangle::new(Point::new(218, 0), Size::new(9, 15))
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                    .draw(&mut display);
                let _ = draw_bluetooth_icon(&mut display, Point::new(219, 1), false);
            }

            // ── E. WiFi ──
            if Some(current_wifi) != last_wifi_icon_state {
                last_wifi_icon_state = Some(current_wifi);
                let _ = Rectangle::new(Point::new(231, 0), Size::new(18, 14))
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                    .draw(&mut display);
                let _ = draw_wifi_icon(&mut display, Point::new(233, 2), current_wifi);
            }

            // ── F. Éclair ──
            let is_external = vsense_volts_val >= 1.0;
            if Some(is_external) != last_lightning_icon_state {
                last_lightning_icon_state = Some(is_external);
                let _ = Rectangle::new(Point::new(253, 0), Size::new(9, 16))
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                    .draw(&mut display);
                let _ = draw_lightning_icon(&mut display, Point::new(254, 0), is_external);
            }

            // ── G. Tension USB ou valeur ──
            let alim_str = if is_external {
                format!("{:<5}", format!("{:.1}V", vsense_volts_val))
            } else {
                "USB  ".to_string()
            };
            let alim_style = if is_external { status_style_white } else { status_style_green };
            let _ = Text::new(&alim_str, Point::new(264, 11), alim_style)
                .draw(&mut display);
        }

        // ── 4. RENDU DE LA BARRE D'ÉTAT (BAS : Y=215..240) ──
        if bottom_changed {
            last_rendered_ina = current_ina;
            last_rendered_inb = current_inb;
            last_rendered_rla = current_rla;
            last_rendered_rlb = current_rlb;
            last_rendered_ssid = current_ssid.clone();
            last_rendered_ip = current_ip.clone();

            let has_power = vsense_volts_val > 6.0;

            // Moteurs INA et INB l'un au-dessus de l'autre à X=2 (Y=223 et Y=233)
            let (ina_text, ina_style) = if has_power {
                if current_ina {
                    (format!("INA: {:<3}%", current_brightness), status_style_green)
                } else {
                    ("INA: 0%  ".to_string(), status_style_red)
                }
            } else {
                ("INA: --  ".to_string(), status_style_gray)
            };
            let ina_padded = format!("{:<9}", ina_text);
            let _ = Text::new(&ina_padded, Point::new(2, 223), ina_style).draw(&mut display);

            let (inb_text, inb_style) = if has_power {
                if current_inb {
                    (format!("INB: {:<3}%", current_brightness), status_style_green)
                } else {
                    ("INB: 0%  ".to_string(), status_style_red)
                }
            } else {
                ("INB: --  ".to_string(), status_style_gray)
            };
            let inb_padded = format!("{:<9}", inb_text);
            let _ = Text::new(&inb_padded, Point::new(2, 233), inb_style).draw(&mut display);

            // Relais RLA et RLB l'un au-dessus de l'autre à X=60 (Y=223 et Y=233)
            let rla_text = if current_rla { "RLA: 1" } else { "RLA: 0" };
            let rla_style = if current_rla { status_style_green } else { status_style_red };
            let rla_padded = format!("{:<6}", rla_text);
            let _ = Text::new(&rla_padded, Point::new(60, 223), rla_style).draw(&mut display);

            let rlb_text = if current_rlb { "RLB: 1" } else { "RLB: 0" };
            let rlb_style = if current_rlb { status_style_green } else { status_style_red };
            let rlb_padded = format!("{:<6}", rlb_text);
            let _ = Text::new(&rlb_padded, Point::new(60, 233), rlb_style).draw(&mut display);

            // SSID et IP (CIDR) l'un au-dessus de l'autre à X=220 (Y=223 et Y=233)
            let ssid_disp = if current_ssid.is_empty() { "--" } else { &current_ssid };
            let ssid_padded = format!("{:<16}", ssid_disp);
            let _ = Text::new(&ssid_padded, Point::new(220, 223), status_style_white).draw(&mut display);

            let ip_disp = if current_ip.is_empty() { "--" } else { &current_ip };
            let ip_padded = format!("{:<16}", ip_disp);
            let _ = Text::new(&ip_padded, Point::new(220, 233), status_style_white).draw(&mut display);
        }

        // ── 5. RENDU DE LA ZONE CENTRALE (MILIEU : Y=20..220) ──
        if middle_changed {
            last_rendered_brightness = current_brightness;
            last_rendered_ver = current_ver;

            // A. Affichage BME280 si présent (Y=80, centré)
            let is_bme_found = crate::i2c::i2c_bme280::BME280_FOUND.load(Ordering::Relaxed);
            let bme_text = if is_bme_found {
                let t = *crate::i2c::i2c_bme280::BME280_TEMP.lock().unwrap();
                let h = *crate::i2c::i2c_bme280::BME280_HUM.lock().unwrap();
                let p = *crate::i2c::i2c_bme280::BME280_PRESS.lock().unwrap();
                format!("BME:{:.1}C {:.1}% {:.0}hPa", t, h, p)
            } else {
                "BME280: Absent".to_string()
            };
            let bme_text_padded = format!("{:<24}", bme_text);
            let bme_style = if is_bme_found { green_style } else { red_style };
            let _ = Text::new(&bme_text_padded, Point::new(40, 80), bme_style)
                .draw(&mut display);

            // B. Luminosité (Y=130, centré)
            let text = format!("Luminosite: {:<3}%", last_rendered_brightness);
            let _ = Text::new(&text, Point::new(80, 130), text_style)
                .draw(&mut display);
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

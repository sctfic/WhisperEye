// screen_display.rs

use embedded_graphics::{
    prelude::*,
    mono_font::{iso_8859_1::FONT_6X10, MonoTextStyleBuilder},
    pixelcolor::Rgb565,
    text::Text,
    primitives::{Rectangle, PrimitiveStyle},
};
use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;
use std::thread;
use common::nvs_storage::NvsStorage;
use crate::screen::{Screen, ST7789Display};
use crate::board::Board;
use crate::actuators::{Actuators, ActuatorsState};
use crate::wifi::NetManager;

// ── 1. FONCTIONS DE DESSIN POUR LES ICÔNES DE STATUT ──

/// Icône WiFi
fn draw_wifi_icon<D>(display: &mut D, start_point: Point, connected: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = if connected { Rgb565::GREEN } else { Rgb565::RED };
    // let border_color = Rgb565::BLACK;
    let base_y = start_point.y + 11;

    // 4 barres : hauteurs 3, 5, 7, 9 px, largeurs 2px, séparées de 1px (i * 3)
    for i in 0..4 {
        let x = start_point.x + (i * 3);
        let height = 3 + (i * 2);
        let y = base_y - height;

        // let _ = Rectangle::new(Point::new(x - 1, y - 1), Size::new(3, (height + 2) as u32))
        //     .into_styled(PrimitiveStyle::with_fill(border_color))
        //     .draw(display);

        let _ = Rectangle::new(Point::new(x, y), Size::new(2, height as u32))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(display);
    }
    Ok(())
}

/// Icône Upload ↗ (Flèche diagonale haut-droite, 7x7 px)
const UPLOAD_ROWS: usize = 7;
const UPLOAD_COLS: usize = 7;
const UPLOAD_BITMAP: [u8; UPLOAD_ROWS] = [
    0b0011111,
    0b0000111,
    0b0001101,
    0b0011001,
    0b0110000,
    0b1100000,
    0b1000000,
];

pub fn draw_upload_icon<D>(display: &mut D, start_point: Point, active: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = if active { Rgb565::GREEN } else { Rgb565::new(15, 30, 15) };
    let x = start_point.x;
    let y = start_point.y;

    for row in 0..UPLOAD_ROWS {
        for col in 0..UPLOAD_COLS {
            let bit = (UPLOAD_BITMAP[row] >> (UPLOAD_COLS - 1 - col)) & 1;
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

/// Icône Download ↙ (Flèche diagonale bas-gauche, 7x7 px)
const DOWNLOAD_ROWS: usize = 7;
const DOWNLOAD_COLS: usize = 7;
const DOWNLOAD_BITMAP: [u8; DOWNLOAD_ROWS] = [
    0b0000001,
    0b0000011,
    0b0000110,
    0b1001100,
    0b1011000,
    0b1110000,
    0b1111100,
];

pub fn draw_download_icon<D>(display: &mut D, start_point: Point, active: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = if active { Rgb565::CYAN } else { Rgb565::new(15, 30, 15) };
    let x = start_point.x;
    let y = start_point.y;

    for row in 0..DOWNLOAD_ROWS {
        for col in 0..DOWNLOAD_COLS {
            let bit = (DOWNLOAD_BITMAP[row] >> (DOWNLOAD_COLS - 1 - col)) & 1;
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

/// Icône Bluetooth
const BT_ROWS: usize = 13;
const BT_COLS: usize = 7;
const BT_BITMAP: [u8; BT_ROWS] = [
    0b0001000,
    0b0001100,
    0b0001010,
    0b1001001,
    0b0101010,
    0b0011100,
    0b0001000,
    0b0011100,
    0b0101010,
    0b1001001,
    0b0001010,
    0b0001100,
    0b0001000,
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

/// Icône I2C
const I2C_ROWS: usize = 14;
const I2C_COLS: usize = 14;
const I2C_BITMAP: [u16; I2C_ROWS] = [
    0b11111111111111,
    0b10000000000001,
    0b00001110000000,
    0b11100010111111,
    0b10101110100001,
    0b10101000101111,
    0b10101110101000,
    0b10100000101000,
    0b10100000101000,
    0b10100000101111,
    0b10100000100001,
    0b11100000111111,
    0b00000000000000,
    0b11111111111111,
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

/// Icône 1-Wire
const OW_ROWS: usize = 8;
const OW_COLS: usize = 15;
const OW_BITMAP: [u16; OW_ROWS] = [
    0b011001000000010,
    0b111001000000010,
    0b011001000000010,
    0b011000100100100,
    0b011000010101000,
    0b111111101010111,
    0b000000000000000,
    0b111111111111111,
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
                    Point::new(x + col as i32, y + row as i32 + 4),
                    color,
                ).draw(display);
            }
        }
    }
    Ok(())
}

/// Icône Éclair
const LIGHTNING_ROWS: usize = 15;
const LIGHTNING_COLS: usize = 7;
const LIGHTNING_BITMAP: [u8; LIGHTNING_ROWS] = [
    0b0000011,
    0b0000101,
    0b0001010,
    0b0001010,
    0b0010010,
    0b0100100,
    0b0101111,
    0b1000001,
    0b1111010,
    0b0010010,
    0b0100100,
    0b0101000,
    0b0101000,
    0b1010000,
    0b1100000,
];

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

/// Icône Warning ⚠ (9x9 px)
const WARNING_ROWS: usize = 9;
const WARNING_COLS: usize = 9;
const WARNING_BITMAP: [u16; WARNING_ROWS] = [
    0b000010000,
    0b000111000,
    0b000101000,
    0b001101100,
    0b001101100,
    0b011111110,
    0b011101110,
    0b111111111,
    0b000000000,
];

fn draw_warning_icon<D>(display: &mut D, start_point: Point) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = Rgb565::new(31, 45, 0); // Orange/Jaune
    let _border_color = Rgb565::BLACK;
    let x = start_point.x;
    let y = start_point.y;

    for row in 0..WARNING_ROWS {
        for col in 0..WARNING_COLS {
            let bit = (WARNING_BITMAP[row] >> (WARNING_COLS - 1 - col)) & 1;
            if bit == 1 {
                let _ = embedded_graphics::Pixel(
                    Point::new(x + col as i32, y + row as i32),
                    color,
                ).draw(display);
            } else {
                let _ = embedded_graphics::Pixel(
                    Point::new(x + col as i32, y + row as i32),
                    Rgb565::BLACK,
                ).draw(display);
            }
        }
    }
    Ok(())
}

/// Icône Check ✓ (9x9 px)
const CHECK_ROWS: usize = 9;
const CHECK_COLS: usize = 9;
const CHECK_BITMAP: [u16; CHECK_ROWS] = [
    0b000000000,
    0b000000010,
    0b000000110,
    0b000001100,
    0b010011000,
    0b011110000,
    0b001100000,
    0b000000000,
    0b000000000,
];

fn draw_check_icon<D>(display: &mut D, start_point: Point) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = Rgb565::GREEN;
    let x = start_point.x;
    let y = start_point.y;

    for row in 0..CHECK_ROWS {
        for col in 0..CHECK_COLS {
            let bit = (CHECK_BITMAP[row] >> (CHECK_COLS - 1 - col)) & 1;
            if bit == 1 {
                let _ = embedded_graphics::Pixel(
                    Point::new(x + col as i32, y + row as i32),
                    color,
                ).draw(display);
            } else {
                let _ = embedded_graphics::Pixel(
                    Point::new(x + col as i32, y + row as i32),
                    Rgb565::BLACK,
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
    nvs_storage: Arc<Mutex<NvsStorage>>,
    wifi_manager: Arc<Mutex<NetManager>>,
) -> Result<(), anyhow::Error> {
    log::info!("Starting Screen IHM thread (screen_display & screen_browse)...");

    let mut last_brightness = {
        let storage = nvs_storage.lock().unwrap();
        storage.get_i32("scrBrightness").ok().flatten().unwrap_or(20) as u32
    };
    let mut timeout_mins;
    let _ = screen.set_backlight(last_brightness);

    let mut last_user_activity = std::time::Instant::now();
    let mut screen_is_off = false;
    let mut last_encoder_val = 0i32;

    let mut controller = crate::screen_browse::BrowseController::new();

    let mut last_btn2_val = true;
    let mut last_btn3_val = true;
    let mut last_ntp_state: Option<bool> = None;
    let mut last_time_str = String::new();

    let mut vsense_volts_val = 0.0f32;
    let mut raw_update_ticks = 0;
    let mut download_visible_until = std::time::Instant::now();
    let mut upload_visible_until = std::time::Instant::now();

    display.clear(Rgb565::BLACK)
        .map_err(|e| anyhow::anyhow!("Clear display error: {:?}", e))?;

    // Ligne séparatrice sous la barre de statut (Y=16)
    let _ = Rectangle::new(Point::new(0, 16), Size::new(320, 1))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::new(10, 20, 10)))
        .draw(&mut display);

    // Ligne séparatrice au-dessus de la barre du bas (Y=219)
    let _ = Rectangle::new(Point::new(0, 219), Size::new(320, 1))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::new(10, 20, 10)))
        .draw(&mut display);

    let status_style_white = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::WHITE).background_color(Rgb565::BLACK).build();
    let status_style_green = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::GREEN).background_color(Rgb565::BLACK).build();
    let status_style_red   = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::RED).background_color(Rgb565::BLACK).build();
    let status_style_gray  = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::new(10, 20, 10)).background_color(Rgb565::BLACK).build();

    let taskbar_ver_style = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::new(10, 20, 10)).build();

    // Rendu statique du titre WhisperEye en bas
    let _ = Rectangle::new(Point::new(130 - 1, 217), Size::new(61, 11))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
        .draw(&mut display);
    let _ = Text::new("WhisperEye", Point::new(130, 225), MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::BLACK).background_color(Rgb565::WHITE).build())
        .draw(&mut display);

    let ver_str = format!("v{}", crate::FW_VERSION);
    let ver_w = ver_str.len() as i32 * 6;
    let ver_x = (320 - ver_w) / 2;
    let _ = Text::new(&ver_str, Point::new(ver_x, 238), taskbar_ver_style)
        .draw(&mut display);

    let mut last_periodic_tick = std::time::Instant::now();

    loop {
        let current_touch = {
            let b = board.lock().unwrap();
            b.is_touch_pressed()
        };

        if current_touch {
            let mut acts = actuators.lock().unwrap();
            let mut st = actuators_state.lock().unwrap();
            if !st.swpwr {
                log::info!("Touche tactile détectée -> Réactivation SWPWR !");
                let _ = acts.write("swpwr", true);
                st.swpwr = true;
            }
        }

        // Lecture de l'état des boutons et de l'encodeur
        let btn2_val = screen.btn2_driver.is_high();
        let btn3_val = screen.btn3_driver.is_high();
        let encoder_count = screen.get_encoder_count();

        let btn2_clicked = last_btn2_val && !btn2_val;
        let btn3_clicked = last_btn3_val && !btn3_val;
        last_btn2_val = btn2_val;
        last_btn3_val = btn3_val;

        // Détecter l'activité utilisateur
        let encoder_delta = encoder_count - last_encoder_val;
        let activity = btn2_clicked || btn3_clicked || (encoder_delta != 0) || current_touch;
        if encoder_count != last_encoder_val {
            last_encoder_val = encoder_count;
        }

        let (brightness_val, timeout_mins_val) = {
            let storage = nvs_storage.lock().unwrap();
            let b = storage.get_i32("scrBrightness").ok().flatten().unwrap_or(20) as u32;
            let t = storage.get_i32("scrTimeout").ok().flatten().unwrap_or(5);
            (b, t)
        };
        timeout_mins = timeout_mins_val;

        if activity {
            last_user_activity = std::time::Instant::now();
            if screen_is_off {
                // Se réveiller
                let _ = screen.set_backlight(brightness_val);
                screen_is_off = false;
                // Ignorer l'entrée de réveil en remettant à zéro les clics et le delta encodeur
                screen.clear_encoder();
                let fresh_encoder = screen.get_encoder_count();
                last_encoder_val = fresh_encoder;
                
                // Synchroniser le contrôleur de navigation pour éviter tout saut
                controller.last_encoder_raw = fresh_encoder;
                controller.encoder_acc = 0;
                
                thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
        }

        if screen_is_off {
            thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }

        // Vérifier le timeout de mise en veille
        let timeout_dur = match timeout_mins {
            1 => Some(std::time::Duration::from_secs(60)),
            5 => Some(std::time::Duration::from_secs(300)),
            30 => Some(std::time::Duration::from_secs(1800)),
            _ => None, // 0 = Jamais
        };

        if let Some(dur) = timeout_dur {
            if last_user_activity.elapsed() > dur {
                let _ = screen.set_backlight(0);
                screen_is_off = true;
                thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
        }

        // Appliquer la luminosité si elle a changé
        if brightness_val != last_brightness {
            let _ = screen.set_backlight(brightness_val);
            last_brightness = brightness_val;
        }

        // Mise à jour de la machine à états de navigation
        controller.process_inputs(
            encoder_count,
            btn2_clicked,
            btn3_clicked,
            &nvs_storage,
            &board,
            &actuators,
            &actuators_state,
            &wifi_manager,
        );

        // Lecture périodique des capteurs de tension / courant
        raw_update_ticks += 1;
        let should_update_raw = raw_update_ticks >= 10;
        if should_update_raw {
            raw_update_ticks = 0;
            let readings = {
                let mut b = board.lock().unwrap();
                let (ina_act, inb_act) = {
                    let act = actuators_state.lock().unwrap();
                    match act.H0.inverseur {
                        -1 => (true, false),
                        1 => (false, true),
                        2 => (act.H0.speed_a > 0, act.H0.speed_b > 0),
                        _ => (false, false),
                    }
                };
                b.read_value(ina_act, inb_act)
            };
            vsense_volts_val = readings.vsense_volts.unwrap_or(0.0);
        }

        let mut periodic_update = false;
        if last_periodic_tick.elapsed() >= std::time::Duration::from_secs(1) {
            last_periodic_tick = std::time::Instant::now();
            periodic_update = true;
        }

        let (current_rla, current_rlb, current_ina, current_inb) = {
            let act = actuators_state.lock().unwrap();
            let (ina_act, inb_act) = match act.H0.inverseur {
                -1 => (true, false),
                1 => (false, true),
                2 => (act.H0.speed_a > 0, act.H0.speed_b > 0),
                _ => (false, false),
            };
            (act.rla, act.rlb, ina_act, inb_act)
        };

        let current_i2c = crate::i2c::I2C_DEVICES_COUNT.load(Ordering::Relaxed);
        let current_onewire = crate::one_wire::ONEWIRE_DEVICES_COUNT.load(Ordering::Relaxed);
        let current_wifi = crate::wifi::WIFI_CONNECTED.load(Ordering::Relaxed);

        let (current_ssid, current_ip) = {
            let ssid = crate::wifi::CURRENT_SSID.lock().unwrap().clone();
            let ip = crate::wifi::CURRENT_IP.lock().unwrap().clone();
            (ssid, ip)
        };

        // NTP & heure UTC
        let ntp_synced = {
            use std::time::{SystemTime, UNIX_EPOCH};
            const YEAR_2020: u64 = 1_577_836_800;
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() > YEAR_2020).unwrap_or(false)
        };

        let current_time_str = {
            use std::time::{SystemTime, UNIX_EPOCH};
            let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let total_mins = secs / 60;
            let hh = (total_mins / 60) % 24;
            let mm = total_mins % 60;
            format!("{:02}:{:02}", hh, mm)
        };

        // ── 1. BARRE DU HAUT (Y=0..16) ──
        if current_time_str != last_time_str || Some(ntp_synced) != last_ntp_state {
            last_time_str = current_time_str.clone();
            last_ntp_state = Some(ntp_synced);

            let _ = Text::new("utc", Point::new(1, 11), status_style_gray).draw(&mut display);
            let _ = Text::new(&current_time_str, Point::new(20, 11), status_style_white).draw(&mut display);

            if !ntp_synced {
                let _ = draw_warning_icon(&mut display, Point::new(54, 3));
            } else {
                let _ = draw_check_icon(&mut display, Point::new(54, 3));
            }
        }

        let _ = draw_i2c_icon(&mut display, Point::new(68, 1), current_i2c > 0);
        let i2c_str = format!("{:>2}", current_i2c);
        let _ = Text::new(&i2c_str, Point::new(82, 11), status_style_white).draw(&mut display);

        let _ = draw_onewire_icon(&mut display, Point::new(102, 1), current_onewire > 0);
        let ow_str = format!("{:>2}", current_onewire);
        let _ = Text::new(&ow_str, Point::new(116, 11), status_style_white).draw(&mut display);

        if current_touch {
            let _ = Rectangle::new(Point::new(230, 2), Size::new(11, 11))
                .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
                .draw(&mut display);
        } else {
            let _ = Rectangle::new(Point::new(230, 2), Size::new(11, 11))
                .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                .draw(&mut display);
        }
        let touch_fill = if current_touch { Rgb565::WHITE } else { Rgb565::new(10, 20, 10) };
        let _ = Rectangle::new(Point::new(232, 4), Size::new(7, 7))
            .into_styled(PrimitiveStyle::with_fill(touch_fill))
            .draw(&mut display);

        let _ = draw_bluetooth_icon(&mut display, Point::new(244, 1), false);

        // Flèches Upload (↗) (-1 Y) et Download (↙) (+2 Y, -2 X) à côté du Wi-Fi (-2px X)
        let now = std::time::Instant::now();
        if crate::wifi::API_DOWNLOAD_ACTIVE.load(Ordering::Relaxed) {
            download_visible_until = now + std::time::Duration::from_millis(500);
        }
        if crate::wifi::API_UPLOAD_ACTIVE.swap(false, Ordering::Relaxed) {
            upload_visible_until = now + std::time::Duration::from_millis(500);
        }
        let is_uploading = now < upload_visible_until;
        let is_downloading = now < download_visible_until;
        let _ = draw_upload_icon(&mut display, Point::new(255, 2), is_uploading);
        let _ = draw_download_icon(&mut display, Point::new(257, 7), is_downloading);

        let _ = draw_wifi_icon(&mut display, Point::new(269, 2), current_wifi);

        let is_external = vsense_volts_val >= 5.0;
        let _ = draw_lightning_icon(&mut display, Point::new(283, 1), is_external);

        let alim_str = if is_external {
            format!("{:<5}", format!("{:.0}V", vsense_volts_val.round()))
        } else {
            "USB  ".to_string()
        };
        let alim_style = if is_external { status_style_white } else { status_style_green };
        let _ = Text::new(&alim_str, Point::new(293, 11), alim_style).draw(&mut display);

        // ── 2. BARRE DU BAS (Y=219..240) ──
        let has_power = vsense_volts_val > 5.0;

        let h0_state = {
            let act = actuators_state.lock().unwrap();
            act.H0.clone()
        };

        // Effacer la zone pour éviter les superpositions de textes précédents
        // let _ = Rectangle::new(Point::new(2, 219), Size::new(55, 20))
        //     .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        //     .draw(&mut display);

        if h0_state.inverseur == 2 {
            // Mode indépendant : afficher A:XX% et B:XX%
            let a_text = if has_power {
                if current_ina { format!("A:{}% ", h0_state.speed_a) } else { "A:0%  ".to_string() }
            } else {
                "A:ERR ".to_string()
            };
            let a_style = if has_power && current_ina { status_style_green } else { status_style_red };
            let _ = Text::new(&format!("{:<9} ", a_text), Point::new(2, 227), a_style).draw(&mut display);

            let b_text = if has_power {
                if current_inb { format!("B:{}% ", h0_state.speed_b) } else { "B:0%  ".to_string() }
            } else {
                "B:ERR ".to_string()
            };
            let b_style = if has_power && current_inb { status_style_green } else { status_style_red };
            let _ = Text::new(&format!("{:<9} ", b_text), Point::new(2, 237), b_style).draw(&mut display);
        } else {
            // Mode inverseur
            let _ = Rectangle::new(Point::new(1, 231), Size::new(50, 8))
                .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                .draw(&mut display);
            if !has_power {
                let _ = Text::new("H:ERR ", Point::new(2, 227), status_style_red).draw(&mut display);
            } else if h0_state.inverseur == -1 && current_ina {
                let _ = Text::new(&format!("   {}% ", h0_state.speed_a), Point::new(0, 227), status_style_green).draw(&mut display);
                let _ = draw_upload_icon(&mut display, Point::new(2, 221), true);
            } else if h0_state.inverseur == 1 && current_inb {
                let _ = Text::new(&format!("   {}% ", h0_state.speed_b), Point::new(0, 227), status_style_green).draw(&mut display);
                let _ = draw_download_icon(&mut display, Point::new(2, 221), true);
            } else {
                let _ = Text::new("H Off ", Point::new(2, 227), status_style_red).draw(&mut display);
            }
        }

        let rla_text = if current_rla { "RLA: 1" } else { "RLA: 0" };
        let rla_style = if current_rla { status_style_green } else { status_style_red };
        let _ = Text::new(&format!("{:<6}", rla_text), Point::new(60, 227), rla_style).draw(&mut display);

        let rlb_text = if current_rlb { "RLB: 1" } else { "RLB: 0" };
        let rlb_style = if current_rlb { status_style_green } else { status_style_red };
        let _ = Text::new(&format!("{:<6}", rlb_text), Point::new(60, 237), rlb_style).draw(&mut display);

        let ssid_disp = if current_ssid.is_empty() { "--" } else { &current_ssid };
        let _ = Text::new(&format!("{:<16}", ssid_disp), Point::new(220, 227), status_style_white).draw(&mut display);

        let ip_disp = if current_ip.is_empty() { "--" } else { &current_ip };
        let _ = Text::new(&format!("{:<16}", ip_disp), Point::new(220, 237), status_style_white).draw(&mut display);

        // ── 3. ZONE CENTRALE (Y=17..218) RENTRÉE PAR SCREEN_BROWSE ──
        let _ = controller.draw(&mut display, &nvs_storage, &board, &actuators, &actuators_state, &wifi_manager, periodic_update);

        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

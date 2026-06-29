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

/// Dessine le symbole Bluetooth classique avec le pixel-art exact fourni par l'utilisateur
fn draw_bluetooth_icon<D>(display: &mut D, start_point: Point, active: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = if active { Rgb565::GREEN } else { Rgb565::new(15, 30, 15) }; // Vert ou Gris
    let border_color = Rgb565::BLACK;
    let x = start_point.x;
    let y = start_point.y;

    const BT_PIXELS: [(i32, i32); 29] = [
        (3, 0),
        (3, 1), (4, 1),
        (3, 2), (5, 2),
        (0, 3), (3, 3), (6, 3),
        (1, 4), (3, 4), (5, 4),
        (2, 5), (3, 5), (4, 5),
        (3, 6),
        (2, 7), (3, 7), (4, 7),
        (1, 8), (3, 8), (5, 8),
        (0, 9), (3, 9), (6, 9),
        (3, 10), (5, 10),
        (3, 11), (4, 11),
        (3, 12)
    ];

    // 1. Dessiner le contour noir autour de chaque pixel du logo
    for &(px, py) in &BT_PIXELS {
        for ox in -1..=1 {
            for oy in -1..=1 {
                let target_x = x + px + ox;
                let target_y = y + py + oy;
                let _ = embedded_graphics::Pixel(Point::new(target_x, target_y), border_color)
                    .draw(display);
            }
        }
    }

    // 2. Dessiner les pixels de l'icône de couleur par-dessus
    for &(px, py) in &BT_PIXELS {
        let _ = embedded_graphics::Pixel(Point::new(x + px, y + py), color)
            .draw(display);
    }

    Ok(())
}

/// Dessine un éclair de couleur jaune (si USB) ou vert (si tension externe) avec contour noir
fn draw_lightning_icon<D>(display: &mut D, start_point: Point, ext_power: bool) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let color = if ext_power { Rgb565::GREEN } else { Rgb565::new(31, 63, 0) }; // Vert ou Jaune
    let border_color = Rgb565::BLACK;
    let x = start_point.x;
    let y = start_point.y;

    let paths = [
        (Point::new(x + 3, y), Point::new(x, y + 5)),
        (Point::new(x, y + 5), Point::new(x + 2, y + 5)),
        (Point::new(x + 2, y + 5), Point::new(x + 1, y + 10)),
        (Point::new(x + 1, y + 10), Point::new(x + 5, y + 4)),
        (Point::new(x + 5, y + 4), Point::new(x + 3, y + 4)),
        (Point::new(x + 3, y + 4), Point::new(x + 3, y)),
    ];

    // Contour noir
    for dx in -1..=1 {
        for dy in -1..=1 {
            if dx == 0 && dy == 0 { continue; }
            for &(p1, p2) in &paths {
                let _ = Line::new(Point::new(p1.x + dx, p1.y + dy), Point::new(p2.x + dx, p2.y + dy))
                    .into_styled(PrimitiveStyle::with_stroke(border_color, 1))
                    .draw(display);
            }
        }
    }

    // Éclair de couleur
    for &(p1, p2) in &paths {
        let _ = Line::new(p1, p2)
            .into_styled(PrimitiveStyle::with_stroke(color, 1))
            .draw(display);
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

    let mut last_wifi_icon_state: Option<bool> = None;
    let mut last_lightning_icon_state: Option<bool> = None;
    let mut last_bt_icon_state: Option<bool> = None;

    let mut vsense_volts_val = 0.0f32;
    let mut isense_amps_val = 0.0f32;

    display.clear(Rgb565::BLACK)
        .map_err(|e| anyhow::anyhow!("Clear display error: {:?}", e))?;

    // Ligne séparatrice sous la barre de statut (Y=13)
    let _ = Rectangle::new(Point::new(0, 13), Size::new(320, 1))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::new(10, 20, 10)))
        .draw(&mut display);

    // Ligne séparatrice au-dessus de la barre d'état (Y=212)
    let _ = Rectangle::new(Point::new(0, 212), Size::new(320, 1))
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

    let mut last_btn2 = true;
    let mut last_btn3 = true;
    let mut btn3_pressed_ticks = 0u32;
    let mut last_brightness = 20i32;

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
        let should_update_raw = raw_update_ticks >= 10;
        
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

        // Déterminer les zones à redessiner
        let status_changed = current_i2c != last_i2c_count
            || current_onewire != last_onewire_count
            || current_wifi != last_wifi_connected
            || current_touch != last_touch_state
            || should_update_raw;

        let bottom_changed = current_ina != last_rendered_ina
            || current_inb != last_rendered_inb
            || current_rla != last_rendered_rla
            || current_rlb != last_rendered_rlb
            || current_ssid != last_rendered_ssid
            || current_ip != last_rendered_ip
            || should_update_raw;

        let middle_changed = current_brightness != last_rendered_brightness
            || current_touch != last_touch_state
            || current_ver != last_rendered_ver;

        // ── 3. RENDU DE LA BARRE DE STATUT (HAUT : Y=0..12) ──
        if status_changed {
            last_i2c_count = current_i2c;
            last_onewire_count = current_onewire;
            last_wifi_connected = current_wifi;
            last_touch_state = current_touch;

            // Texte à gauche : I2C et 1-Wire (Y=9) avec le petit "2" dessiné à la main
            let _ = Text::new(" I", Point::new(5, 9), status_style_white).draw(&mut display);
            
            // Effacer la petite zone du "2" avant dessin (décalée de 1px à gauche : X=16)
            let _ = Rectangle::new(Point::new(16, 0), Size::new(3, 12))
                .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                .draw(&mut display);
                
            // Dessiner le petit "2" de 5px de haut et 3px de large (décalé de 1px à gauche)
            let pixel_color = status_style_white.text_color.unwrap_or(Rgb565::WHITE);
            let _ = embedded_graphics::Pixel(Point::new(17, 1), pixel_color).draw(&mut display);
            let _ = embedded_graphics::Pixel(Point::new(18, 1), pixel_color).draw(&mut display);
            let _ = embedded_graphics::Pixel(Point::new(16, 2), pixel_color).draw(&mut display);
            let _ = embedded_graphics::Pixel(Point::new(18, 2), pixel_color).draw(&mut display);
            let _ = embedded_graphics::Pixel(Point::new(17, 3), pixel_color).draw(&mut display);
            let _ = embedded_graphics::Pixel(Point::new(16, 4), pixel_color).draw(&mut display);
            let _ = embedded_graphics::Pixel(Point::new(16, 5), pixel_color).draw(&mut display);
            let _ = embedded_graphics::Pixel(Point::new(17, 5), pixel_color).draw(&mut display);
            let _ = embedded_graphics::Pixel(Point::new(18, 5), pixel_color).draw(&mut display);

            let _ = Text::new("C", Point::new(20, 9), status_style_white).draw(&mut display);
            let val_text = format!(" {:<2} ", current_i2c);
            let _ = Text::new(&val_text, Point::new(26, 9), status_style_white).draw(&mut display);

            let ow_text = format!(" 1-Wire {:<2} ", current_onewire);
            let _ = Text::new(&ow_text, Point::new(55, 9), status_style_white)
                .draw(&mut display);

            // TOUCH en texte police 6x10 (Blanc sur fond bleu si actif, sinon gris sur fond noir)
            let touch_style = if current_touch { touch_style_active } else { status_style_gray };
            let _ = Text::new("TOUCH", Point::new(125, 9), touch_style)
                .draw(&mut display);

            // Rendu icône Bluetooth (10px avant le wifi : X=242, Y=1)
            if last_bt_icon_state.is_none() {
                last_bt_icon_state = Some(false);
                let _ = Rectangle::new(Point::new(240, 0), Size::new(12, 12))
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                    .draw(&mut display);
                let _ = draw_bluetooth_icon(&mut display, Point::new(242, 1), false);
            }

            // Rendu icône WiFi (X=260, Y=1)
            if Some(current_wifi) != last_wifi_icon_state {
                last_wifi_icon_state = Some(current_wifi);
                let _ = Rectangle::new(Point::new(258, 0), Size::new(18, 12))
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                    .draw(&mut display);
                let _ = draw_wifi_icon(&mut display, Point::new(260, 1), current_wifi);
            }

            // Rendu icône Alimentation / Éclair (X=280, Y=1)
            let is_external = vsense_volts_val >= 1.0;
            if Some(is_external) != last_lightning_icon_state {
                last_lightning_icon_state = Some(is_external);
                let _ = Rectangle::new(Point::new(278, 0), Size::new(12, 12))
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                    .draw(&mut display);
                let _ = draw_lightning_icon(&mut display, Point::new(280, 1), is_external);
            }

            // Annotation Alimentation (X=290, Y=9)
            let alim_str = if is_external {
                format!("{:<5}", format!("{:.1}V", vsense_volts_val))
            } else {
                "USB  ".to_string()
            };
            let alim_style = if is_external { status_style_white } else { status_style_green };
            let _ = Text::new(&alim_str, Point::new(290, 9), alim_style)
                .draw(&mut display);
        }

        // ── 4. RENDU DE LA BARRE D'ÉTAT (BAS : Y=215..235) ──
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

        // ── 5. RENDU DE LA ZONE CENTRALE (MILIEU : Y=18..211) ──
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

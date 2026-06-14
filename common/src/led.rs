use std::sync::OnceLock;

static NEOPIXEL: OnceLock<NeoPixel> = OnceLock::new();

pub struct NeoPixel {
    t0h_cycles: u32,
    t0l_cycles: u32,
    t1h_cycles: u32,
    t1l_cycles: u32,
}

impl NeoPixel {
    pub fn new() -> Self {
        // Fréquence CPU ESP32-S3 à 240 MHz (240 cycles par microseconde)
        // T0H : 350 ns -> 0.35 * 240 = 84 cycles
        // T0L : 800 ns -> 0.80 * 240 = 192 cycles
        // T1H : 700 ns -> 0.70 * 240 = 168 cycles
        // T1L : 600 ns -> 0.60 * 240 = 144 cycles
        Self {
            t0h_cycles: 84,
            t0l_cycles: 192,
            t1h_cycles: 168,
            t1l_cycles: 144,
        }
    }

    pub fn init(&self) {
        let pin_mask = 1u64 << 48;
        let config = esp_idf_sys::gpio_config_t {
            pin_bit_mask: pin_mask,
            mode: esp_idf_sys::gpio_mode_t_GPIO_MODE_OUTPUT,
            pull_up_en: esp_idf_sys::gpio_pullup_t_GPIO_PULLUP_DISABLE,
            pull_down_en: esp_idf_sys::gpio_pulldown_t_GPIO_PULLDOWN_DISABLE,
            intr_type: esp_idf_sys::gpio_int_type_t_GPIO_INTR_DISABLE,
        };
        unsafe {
            esp_idf_sys::gpio_config(&config);
            // Mettre à bas au départ (GPIO 38 -> bit 6 du registre de la 2ème banque de GPIO)
            core::ptr::write_volatile(0x6000_401c as *mut u32, 1 << (38 - 32));
        }
    }

    pub fn set_color(&self, r: u8, g: u8, b: u8) {
        // Limiter la puissance à 10% (max 25 sur 255)
        let r = r.min(25);
        let g = g.min(25);
        let b = b.min(25);
        
        let color_data: u32 = ((g as u32) << 16) | ((r as u32) << 8) | (b as u32);
        
        let ps = disable_interrupts();

        for i in (0..24).rev() {
            let bit = (color_data >> i) & 1;
            if bit == 1 {
                unsafe {
                    core::ptr::write_volatile(0x6000_4010 as *mut u32, 1 << (38 - 32));
                }
                delay_cycles(self.t1h_cycles);
                unsafe {
                    core::ptr::write_volatile(0x6000_401c as *mut u32, 1 << (38 - 32));
                }
                delay_cycles(self.t1l_cycles);
            } else {
                unsafe {
                    core::ptr::write_volatile(0x6000_4010 as *mut u32, 1 << (38 - 32));
                }
                delay_cycles(self.t0h_cycles);
                unsafe {
                    core::ptr::write_volatile(0x6000_401c as *mut u32, 1 << (38 - 32));
                }
                delay_cycles(self.t0l_cycles);
            }
        }


        restore_interrupts(ps);

        // Attendre 80 us (WS2812 Reset)
        unsafe {
            esp_idf_sys::esp_rom_delay_us(80);
        }
    }
}

#[inline(always)]
fn get_ccount() -> u32 {
    let mut count: u32;
    unsafe {
        core::arch::asm!("rsr.ccount {0}", out(reg) count);
    }
    count
}

#[inline(always)]
fn delay_cycles(cycles: u32) {
    let start = get_ccount();
    while get_ccount().wrapping_sub(start) < cycles {}
}

#[inline(always)]
fn disable_interrupts() -> u32 {
    let mut ps: u32;
    unsafe {
        core::arch::asm!(
            "rsr.ps {0}",
            "rsil {0}, 15",
            out(reg) ps
        );
    }
    ps
}

#[inline(always)]
fn restore_interrupts(ps: u32) {
    unsafe {
        core::arch::asm!(
            "wsr.ps {0}",
            "esync",
            in(reg) ps
        );
    }
}

pub fn set_led_color(r: u8, g: u8, b: u8) {
    let neopixel = NEOPIXEL.get_or_init(|| {
        let np = NeoPixel::new();
        np.init();
        np
    });
    neopixel.set_color(r, g, b);
}

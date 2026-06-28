use esp_idf_hal::gpio::*;
use esp_idf_hal::spi::*;
use esp_idf_hal::units::FromValueType;
use esp_idf_hal::delay::Ets;
use display_interface_spi::SPIInterfaceNoCS;
use mipidsi::{Builder, ColorInversion};
use embedded_graphics::{
    prelude::*,
    mono_font::{ascii::FONT_10X20, MonoTextStyle},
    text::Text,
    pixelcolor::Rgb565,
};

/// Check screen presence.
pub fn is_present() -> bool {
    // Screen is always connected on production board
    true
}

pub fn init_screen(
    spi: esp_idf_hal::spi::SPI2,
    sclk: Gpio7,
    sda: Gpio15,
    rst: Gpio16,
    dc: Gpio4,
    cs: Gpio5,
    blk: Gpio6,
) -> Result<(), anyhow::Error> {
    // 1. Activer le rétroéclairage
    let mut blk_driver = PinDriver::output(blk)?;
    blk_driver.set_high()?;

    // 2. Configurer le bus SPI
    let bus_config = SpiDriverConfig::default();
    let config = SpiConfig::new()
        .baudrate(26.MHz().into());
    let device = SpiDeviceDriver::new_single(
        spi,
        sclk,
        sda,
        Option::<Gpio0>::None,
        Some(cs),
        &bus_config,
        &config,
    )?;

    // 3. Interface d'affichage
    let dc_driver = PinDriver::output(dc)?;
    let di = SPIInterfaceNoCS::new(device, dc_driver);

    // 4. Reset physique du ST7789
    let mut rst_driver = PinDriver::output(rst)?;
    rst_driver.set_high()?;
    Ets::delay_us(5000);
    rst_driver.set_low()?;
    Ets::delay_us(15000);
    rst_driver.set_high()?;
    Ets::delay_us(15000);

    // 5. Initialiser le driver ST7789 via mipidsi
    let mut delay = Ets;
    let mut display = Builder::st7789(di)
        .with_display_size(240, 320)
        .with_orientation(mipidsi::Orientation::Landscape(true))
        .with_invert_colors(ColorInversion::Inverted)
        .init(&mut delay, Some(rst_driver))
        .map_err(|e| anyhow::anyhow!("Display init error: {:?}", e))?;

    // 6. Effacer l'écran en noir
    display.clear(Rgb565::BLACK)
        .map_err(|e| anyhow::anyhow!("Clear display error: {:?}", e))?;

    // 7. Dessiner le "Hello World!"
    let text_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    Text::new("Hello World!", Point::new(100, 120), text_style)
        .draw(&mut display)
        .map_err(|e| anyhow::anyhow!("Draw text error: {:?}", e))?;

    Ok(())
}

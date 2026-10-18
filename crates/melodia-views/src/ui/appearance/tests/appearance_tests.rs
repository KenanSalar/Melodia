use melodia_core::error::AppError;
use melodia_core::themes::SYSTEM_VARIANT_ID;
use melodia_core::themes::material3::MATERIAL3;

use super::*;

/// Material 3 without its System chip. Every shipped theme offers one, so the partition has no
/// real member to stand in for it.
fn without_system() -> ThemeDef {
    ThemeDef { supports_system_mode: false, ..MATERIAL3 }
}

// Both ends of the variant run, where an off-by-one shows.
#[test]
fn a_variant_chip_names_its_variant() -> Result<(), AppError> {
    let (Some(first), Some(last)) = (MATERIAL3.variants.first(), MATERIAL3.variants.last()) else {
        return Err(AppError::NotFound("Material 3 ships no variants".to_owned()));
    };

    assert_eq!(variant_id_at(&MATERIAL3, 0), Some(first.id));
    assert_eq!(variant_id_at(&MATERIAL3, MATERIAL3.variants.len() - 1), Some(last.id));
    Ok(())
}

#[test]
fn the_chip_past_the_variants_is_system() {
    assert_eq!(variant_id_at(&MATERIAL3, MATERIAL3.variants.len()), Some(SYSTEM_VARIANT_ID));
}

#[test]
fn a_theme_without_system_has_no_chip_past_its_variants() {
    let theme = without_system();

    assert_eq!(variant_id_at(&theme, theme.variants.len()), None);
}

// A stale index from a theme with more variants picks nothing rather than a neighbour.
#[test]
fn no_chip_sits_past_system() {
    assert_eq!(variant_id_at(&MATERIAL3, MATERIAL3.variants.len() + 1), None);
}

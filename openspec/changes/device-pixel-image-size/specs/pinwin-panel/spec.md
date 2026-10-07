# Spec Delta — pinwin-panel

## ADDED Requirements

### Requirement: Kitty images drawn at device resolution
A kitty image whose drawn size comes from its own pixel size SHALL be drawn at that size in
device pixels: its logical size is the image's pixel size divided by the panel's resolved
output scale, rounded to a whole logical pixel. This applies to unicode-placeholder
placements and to placements that give neither a column nor a row count. A placement that
gives a column or row count SHALL keep the size its cells give it.

#### Scenario: Placeholder image at a fractional scale
- **WHEN** the panel runs at an output scale of 1.8 and the host's child shows a 576×324
  pixel image through unicode placeholders
- **THEN** the image is drawn 320×180 logical pixels (576×324 device pixels) and stays
  inside the panel

#### Scenario: Image-sized placement at a fractional scale
- **WHEN** the panel runs at an output scale of 1.8 and the host's child places a 576×324
  pixel image with no column or row count
- **THEN** the image is drawn 320×180 logical pixels

#### Scenario: Cell-sized placement is unchanged
- **WHEN** the panel runs at an output scale of 1.8 and the host's child places an image
  with a column count of 10 and a row count of 5
- **THEN** the image is drawn 10 cells wide and 5 cells high, the same as at scale 1

#### Scenario: Scale 1 is unchanged
- **WHEN** the panel runs at an output scale of 1 and shows a 320×180 pixel image through
  unicode placeholders
- **THEN** the image is drawn 320×180 logical pixels

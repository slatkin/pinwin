# Spec Delta

## MODIFIED Requirements

### Requirement: Run options
`pinwin run` SHALL accept `--name <name>`, `--edge <left|right|top|bottom>`,
`--size <size>`, `--reserve <push|cover|none>`, `--layer <background|bottom|top|overlay>`,
`--keyboard <none|on-demand|exclusive>`, `--output <output name>`, `--hidden`,
`--focus-ring <off|#RRGGBB[:width]>`, `--gutter-top <pixels>`, `--gutter-bottom <pixels>`,
`--gutter-left <pixels>`, `--gutter-right <pixels>` and `--show-existing`. A gutter value
SHALL be a whole number of logical pixels and MAY be negative. `--` SHALL end the options. A
missing command, an unknown option or an invalid value SHALL make pinwin print a message and
exit 2.

#### Scenario: Options apply to the new panel
- **WHEN** the user runs `pinwin run --name notes --edge right --size 30% -- myeditor`
- **THEN** the `notes` panel docks at the right edge with a width of 30% of its output

#### Scenario: Invalid option value
- **WHEN** the user runs `pinwin run --edge middle -- myeditor`
- **THEN** pinwin prints a message, exits 2, and no panel is created

#### Scenario: No command
- **WHEN** the user runs `pinwin run --name notes`
- **THEN** pinwin prints a message and exits 2

#### Scenario: Start hidden
- **WHEN** the user runs `pinwin run --name notes --hidden -- myeditor`
- **THEN** the app runs, the panel does not show, and a later `pinwin show notes` shows it

#### Scenario: Gutter option wins over the file
- **WHEN** the user runs `pinwin run --name notes --gutter-left 12 -- myeditor` and
  `[panel.notes]` sets `gutter_left = 4`
- **THEN** the panel starts 12 pixels from the left output edge

#### Scenario: Reserve none at start
- **WHEN** the user runs `pinwin run --name notes --reserve none -- myeditor`
- **THEN** the `notes` panel starts with reserve mode `none` and reserves nothing

### Requirement: Size and reserve in one request
`set-size` and `resize` SHALL accept an optional `reserve` field, `push`, `cover` or `none`.
The daemon SHALL validate the new size and the new reserve mode together, with the rules for the
new reserve mode, and SHALL apply them as one change with the request's `ms` or the panel's
`tween_ms`. A refusal SHALL change neither the size nor the reserve mode. A `reserve` equal
to the panel's mode SHALL change only the size. A request without `reserve` SHALL behave as
before.

#### Scenario: Covering expand in one request
- **WHEN** a pushing panel 400 pixels wide gets
  `{"cmd":"set-size","panel":"notes","size":"1100px","reserve":"cover","ms":200}`
- **THEN** the panel grows to 1100 pixels over about 200 ms, the strip stays at 400 pixels,
  tiled windows do not move, and `status` reports the size `1100px` and the mode `cover`

#### Scenario: Refused together
- **WHEN** a covering panel gets a `set-size` with `reserve` `push` whose strip leaves no
  output space for other windows
- **THEN** the reply has error kind `invalid_layout`, and the size and the reserve mode do not
  change

#### Scenario: Same reserve mode
- **WHEN** a covering panel gets `set-size` with `"reserve":"cover"` and a new size
- **THEN** only the size changes, as with a `set-size` without `reserve`

#### Scenario: Relative resize with a mode
- **WHEN** a covering panel 1100 pixels wide gets
  `{"cmd":"resize","panel":"notes","by":"-700px","reserve":"push"}`
- **THEN** the panel becomes 400 pixels wide and pushes

#### Scenario: Invalid mode
- **WHEN** a client sends a `set-size` with `"reserve":"middle"`
- **THEN** the reply has error kind `invalid_value` and the panel does not change

#### Scenario: Release the strip of a running panel
- **WHEN** a pushing panel with a 400 pixel strip gets `{"cmd":"set-reserve","panel":"notes","reserve":"none"}`
- **THEN** the reply is a success, the strip becomes zero, and `status` reports the mode `none`

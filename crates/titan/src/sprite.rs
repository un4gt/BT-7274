use super::{
    paint::{ARMOR, BLACK, Canvas, DIM, OLIVE, RUST, STEEL, mix},
    timeline::{APPEAR_MS, IGNITION_MS, IMPACT_MS, Phase, Timeline, progress},
};

// Only the mechanical parts are artwork. @ is an anchor, never a painted eye.
// Raised missile racks, radiator shoulders, recessed sensor, split chest armor.
const BODY: &[&str] = &[
    r"      .-------.                           .-------.",
    r"      |:o:o:o:|\                         /|:o:o:o:|",
    r"      |_______|/                         \|_______|",
    r"          \\  \          ___          /  //",
    r"           \\__\________/___\________/__//",
    r"      .-----\===\______/__|__\______/===/-----.",
    r"     /####/|#####|  __/_______\__  |#####|\####\",
    r"    /|===| |#####| / /  .---.  \ \ |#####| |===|\",
    r"    ||:::| |_____| | | [ @ ] | | | |_____| |:::||",
    r"    ||___|/  /  /| |  '---'  | |\  \  \|___||",
    r"    /| o |  /  / |__\_______/__| \  \  | o |\",
    r"   / |___| /  / /%%%/ V \%%%\ \ \  \ |___| \",
    r"   |/===/| | | |%%%/ /2\ \%%%| | | | |\===\|",
    r"   ||::| | | |  \%/_______\%/  | | | | |::||",
    r"   ||__|/  \  \__\_BT-7274_/__/  /  \|__||",
    r"   /___/    '---[==|:::::|==]---'    \___\",
];

const LEGS_STANDING: &[&str] = &[
    r"             /%%%/\_______/\%%%\",
    r"            /%%%/  |:::::|  \%%%\",
    r"           /___/|  '-----'  |\___\",
    r"          [===] /           \ [===]",
    r"          |:::|/             \|:::|",
    r"          |___|               |___|",
    r"         /%%/|                 |\%%\",
    r"        /__/ |                 | \__\",
    r"        |::|/                   \|::|",
    r"     __/___|                     |___\__",
    r"    /__|_|_\                     /_|_|__\",
];

const LEGS_KNEELING: &[&str] = &[
    r"          /%%%%/\______/\%%%%\____",
    r"     ____/____/  '----'  \___[====]",
    r"    /%%%[====]           \___\::::\",
    r"    |___|::::|              \__\___\___",
    r"   /__|_|____\               /___|_|___\",
];

const LEGS_RISING: &[&str] = &[
    r"            /%%%/\_____/\%%%\",
    r"         __/%%%/  '---'  \%%%\",
    r"        /____/|          |\___\",
    r"        [===] /          \ [===]",
    r"        |:::|/            \|:::|",
    r"        |___|              \___\",
    r"       /%%/|                |\%%\",
    r"    __/___/                  \___\___",
    r"   /__|_|_\                   /_|_|__\",
];

const LEGS_FALLING: &[&str] = &[
    r"            /%%%/\_____/\%%%\",
    r"           [===]  '---'  [===]",
    r"            \::\        /::/",
    r"             \__\      /__/",
    r"            /_|_|      |_|_\",
];

// A separately drawn compact chassis keeps joints and the sensor legible at 80x24.
const COMPACT_BODY: &[&str] = &[
    r"    [:::]                 [:::]",
    r"      \\_____ _____ _____//",
    r"   .---\===/_/_____\_\===/---.",
    r"  /###/|###| / ___ \ |###|\###\",
    r"  |===||___|| [ @ ] ||___||===|",
    r"  |:o:| / / |__\_/__| \ \ |:o:|",
    r"  /__/| | | /%%/V\%%\ | | |\__\",
    r"  |::|| | | \%/_2_\%/ | | ||::|",
    r"  |__|/  \__\BT-7274/__/  \|__|",
    r" /___/    [==|:::|==]      \___\",
];
const COMPACT_STANDING: &[&str] = &[
    r"        /%%/\___/\%%\",
    r"       /__/  :::  \__\",
    r"      [==]/       \[==]",
    r"      |::|         |::|",
    r"     /__/|         |\__\",
    r"   _/___/           \___\_",
    r"  /_|_|_\           /_|_|_\",
];
const COMPACT_KNEELING: &[&str] = &[
    r"     /%%/\____/\%%\___",
    r"   _[==]  '--'  \_[===]",
    r"  /_|_|_\          \_|_|__\",
];
const COMPACT_RISING: &[&str] = &[
    r"      /%%/\___/\%%\",
    r"    _[==]  :::  [==]",
    r"   /_::/        \::|",
    r"   |__/          \__\",
    r"  /_|_|_\         /_|_|_\",
];

// 80x24 聊天框的消息区通常不足 10 行；独立小造型保留肩甲和传感器，避免抽样丢失结构。
const MINI_BODY: &[&str] = &[
    r"   [::]       [::]",
    r"  /###\__/^\__/###\",
    r"  |:::| [ @ ] |:::|",
    r"  |___|\_7274_/|___|",
    r" /___/ [==:==] \___\",
];
const MINI_STANDING: &[&str] = &[r"    /#/\_/\#\", r"   [=]/   \[=]", r"  /__|     |__\"];
const MINI_KNEELING: &[&str] = &[r"   /#/\__/\#\__", r" /__|      \_|_\"];

#[derive(Clone, Copy, Debug)]
pub(crate) enum Format {
    Detailed,
    Compact,
    Mini,
}

#[derive(Clone, Copy, Debug)]
pub struct Chassis {
    pub origin: (i32, i32),
    pub format: Format,
    pub eye: (i32, i32),
    pub left_nozzle: (i32, i32),
    pub right_nozzle: (i32, i32),
    pub bottom: i32,
    pub scale: f32,
}

pub fn draw(canvas: &mut Canvas, time: Timeline, ground: i32, detailed: bool) -> Chassis {
    let mini = !detailed && (canvas.height < 20 || canvas.width < 38);
    let body = if detailed {
        BODY
    } else if mini {
        MINI_BODY
    } else {
        COMPACT_BODY
    };
    let width = body.iter().map(|row| row.len()).max().unwrap_or(0) as i32;
    let standing_height = body.len() as f32
        + if detailed {
            11.0
        } else if mini {
            3.0
        } else {
            7.0
        };
    let fit = ((canvas.width - 4).max(1) as f32 / width as f32)
        .min((ground - if mini { 0 } else { 2 }).max(1) as f32 / standing_height)
        .min(1.0);
    let legs = if mini {
        if time.is_airborne() || time.rise < 0.5 {
            MINI_KNEELING
        } else {
            MINI_STANDING
        }
    } else if time.is_airborne() {
        if detailed {
            LEGS_FALLING
        } else {
            COMPACT_KNEELING
        }
    } else if time.rise < 0.30 {
        if detailed {
            LEGS_KNEELING
        } else {
            COMPACT_KNEELING
        }
    } else if time.rise < 0.83 {
        if detailed {
            LEGS_RISING
        } else {
            COMPACT_RISING
        }
    } else if detailed {
        LEGS_STANDING
    } else {
        COMPACT_STANDING
    };

    let max_leg_height = if detailed {
        11.0
    } else if mini {
        3.0
    } else {
        7.0
    };
    let min_leg_height = if detailed {
        5.0
    } else if mini {
        2.0
    } else {
        3.0
    };
    let leg_height =
        (min_leg_height + (max_leg_height - min_leg_height) * time.rise).round() as i32;
    let (scale, bottom) = if time.phase == Phase::Falling {
        let t = progress(time.ms, APPEAR_MS, IGNITION_MS);
        let scale = 0.10 + 0.90 * t.powf(1.7);
        let falling_height = (body.len() as f32 + min_leg_height) * fit;
        let final_center = ground as f32 - 7.0 * fit - falling_height / 2.0;
        // Keep the first tiny silhouette visible at 100ms while it grows toward us.
        let center = 1.0 + (final_center - 1.0) * t * t;
        (scale * fit, center + falling_height * scale / 2.0)
    } else if time.phase == Phase::Braking {
        let t = progress(time.ms, IGNITION_MS, IMPACT_MS);
        // Sustain the retro burn above the ground; contact stays a distinct impact.
        (fit, ground as f32 - fit - 6.0 * fit * (1.0 - t).powi(3))
    } else {
        (fit, ground as f32)
    };

    let height = body.len() as i32 + leg_height;
    let origin = (
        canvas.width / 2 - (width as f32 * scale / 2.0).round() as i32,
        bottom.round() as i32 - (height as f32 * scale).round() as i32,
    );
    let brightness = if time.is_airborne() {
        (scale * scale).max(0.12)
    } else {
        1.0
    };

    paint_part(
        canvas,
        body,
        origin,
        width,
        body.len() as i32,
        scale,
        brightness,
    );
    // The hip lifts continuously; leg components extend between two planted feet.
    let leg_width = legs.iter().map(|row| row.len()).max().unwrap_or(0) as i32;
    let leg_origin = (
        origin.0 + (((width - leg_width) / 2) as f32 * scale).round() as i32,
        origin.1 + (body.len() as f32 * scale).round() as i32,
    );
    paint_part(
        canvas, legs, leg_origin, leg_width, leg_height, scale, brightness,
    );

    if !time.is_airborne() && time.rise < 0.30 && detailed {
        // Grounded fist: a third point of contact during the impact hold.
        for y in (origin.1 + body.len() as i32)..ground - 1 {
            canvas.text(origin.0 + 3, y, "|::|", STEEL);
        }
        canvas.text(origin.0 + 1, ground - 1, "[__|__]", ARMOR);
    }

    let (eye_y, eye_x) = body
        .iter()
        .enumerate()
        .find_map(|(y, row)| row.find('@').map(|x| (y, x)))
        .expect("sensor anchor in BT sprite");
    Chassis {
        origin,
        format: if detailed {
            Format::Detailed
        } else if mini {
            Format::Mini
        } else {
            Format::Compact
        },
        eye: (
            origin.0 + (eye_x as f32 * scale).round() as i32,
            origin.1 + (eye_y as f32 * scale).round() as i32,
        ),
        left_nozzle: (
            origin.0 + (9.0 * scale).round() as i32,
            origin.1 + ((body.len() as f32 - 5.0) * scale).round() as i32,
        ),
        right_nozzle: (
            origin.0 + ((width as f32 - 9.0) * scale).round() as i32,
            origin.1 + ((body.len() as f32 - 5.0) * scale).round() as i32,
        ),
        bottom: bottom.round() as i32,
        scale,
    }
}

fn paint_part(
    canvas: &mut Canvas,
    rows: &[&str],
    origin: (i32, i32),
    width: i32,
    height: i32,
    scale: f32,
    brightness: f32,
) {
    let out_width = (width as f32 * scale).round().max(1.0) as i32;
    let out_height = (height as f32 * scale).round().max(1.0) as i32;
    for dy in 0..out_height {
        let sy = (dy as usize * rows.len() / out_height as usize).min(rows.len() - 1);
        for dx in 0..out_width {
            let sx = dx as usize * width as usize / out_width as usize;
            let ch = rows[sy].as_bytes().get(sx).copied().unwrap_or(b' ') as char;
            if ch == ' ' || ch == '@' {
                // Matte out the inner chassis so smoke cannot shine through armor.
                let first = rows[sy].len() - rows[sy].trim_start().len();
                if sx >= first && sx < rows[sy].len() {
                    canvas.put(origin.0 + dx, origin.1 + dy, ' ', BLACK);
                }
                continue;
            }
            let (glyph, color) = match ch {
                '%' => ('#', ARMOR),
                '#' => ('#', OLIVE),
                '=' => ('=', RUST),
                ':' | 'o' => (ch, DIM),
                'V' | '2' => (ch, RUST),
                'A'..='Z' | '0'..='9' => (ch, ARMOR),
                _ => (ch, STEEL),
            };
            canvas.put(
                origin.0 + dx,
                origin.1 + dy,
                glyph,
                mix(BLACK, color, brightness),
            );
            let shade = match ch {
                '%' => 0.18,
                '#' => 0.13,
                '=' => 0.24,
                _ => 0.0,
            };
            if shade > 0.0 {
                canvas.tint_background(origin.0 + dx, origin.1 + dy, color, shade * brightness);
            }
        }
    }
}

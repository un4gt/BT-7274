"""Render actual TUI frames exported by the ignored export_titan_preview Rust test."""

import json
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


ROOT = Path(__file__).resolve().parents[1] / "target" / "titan-preview"
FONTS = [
    Path("C:/Windows/Fonts/consola.ttf"),
    Path("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"),
    Path("/System/Library/Fonts/Menlo.ttc"),
]
FONT = next(path for path in FONTS if path.exists())
SYMBOL = Path("C:/Windows/Fonts/seguisym.ttf")
font = ImageFont.truetype(str(FONT), 16)
symbol_font = ImageFont.truetype(str(SYMBOL if SYMBOL.exists() else FONT), 14)
label_font = ImageFont.truetype(str(FONT), 17)

HTML = """<!doctype html><html lang="zh"><meta charset="utf-8">
<title>BT-7274 启动动画预览</title>
<style>
body{background:#101719;color:#d7e9e6;font:15px system-ui;margin:28px auto;max-width:1240px}
canvas{display:block;max-width:100%;height:auto;border:1px solid #304b4c;margin:20px auto}
button{background:#163234;color:#e0efed;border:1px solid #2dd4bf;padding:8px 18px;cursor:pointer}
input{width:60%;vertical-align:middle;margin:0 18px} p{color:#809795}
</style><h2>BT-7274 · 降落 → 震屏 → 聊天界面</h2>
<p>真实 Ratatui 绘制结果 · 拖动时间轴查看每一帧</p>
<button id="play">暂停</button><input id="seek" type="range" min="0" max="__DURATION__" value="0"><span id="time"></span>
<canvas id="screen"></canvas>
<script id="frames" type="application/json">__DATA__</script>
<script>
const data=JSON.parse(document.getElementById('frames').textContent), canvas=document.getElementById('screen'),ctx=canvas.getContext('2d');
const play=document.getElementById('play'),seek=document.getElementById('seek'),time=document.getElementById('time');
const cw=10,ch=20;canvas.width=data.width*cw;canvas.height=data.height*ch;
let playing=true,ms=0,last=performance.now();
function draw(){ctx.font='16px Consolas, monospace';ctx.textBaseline='middle';
 const index=Math.min(data.frames.length-1,Math.round(ms*data.fps/1000));
 for(const [x,y,fg,bg,text] of data.frames[index]){ctx.fillStyle=bg;ctx.fillRect(x*cw,y*ch,[...text].length*cw,ch);ctx.fillStyle=fg;
 let offset=0;for(const glyph of text){ctx.fillText(glyph,(x+offset)*cw,y*ch+ch/2);offset++;}}
 seek.value=ms;time.textContent=(ms/1000).toFixed(2)+' / '+(data.duration/1000).toFixed(2)+' s';}
play.onclick=()=>{if(ms>=data.duration){ms=0;playing=true;}else{playing=!playing;}play.textContent=playing?'暂停':'播放';last=performance.now();};
seek.oninput=()=>{ms=Number(seek.value);playing=false;play.textContent='播放';draw();};
function tick(now){if(playing){ms=Math.min(data.duration,ms+now-last);if(ms>=data.duration){playing=false;play.textContent='重播';}draw();}last=now;requestAnimationFrame(tick);}
draw();requestAnimationFrame(tick);
</script></html>"""


def export(path):
    data = json.loads(path.read_text(encoding="utf-8"))

    def render(ms):
        index = min(len(data["frames"]) - 1, round(ms * data["fps"] / 1000))
        image = Image.new("RGB", (data["width"] * 10, data["height"] * 20))
        draw = ImageDraw.Draw(image)
        for x, y, fg, bg, text in data["frames"][index]:
            draw.rectangle((x * 10, y * 20, (x + len(text)) * 10 - 1, (y + 1) * 20 - 1), fill=bg)
            for dx, glyph in enumerate(text):
                if glyph in "●◉":
                    draw.text(((x + dx + 0.5) * 10, y * 20 + 10), glyph, font=symbol_font, fill=fg, anchor="mm")
                else:
                    draw.text(((x + dx) * 10, y * 20 + 10), glyph, font=font, fill=fg, anchor="lm")
        return image

    steps = [(500, "DESCENT"), (1350, "IMPACT"), (2750, "HANDOFF START"), (3150, "EASE IN"),
             (3550, "MOVE & SCALE"), (3950, "CHAT REVEAL"), (4400, "EASE OUT"), (data["duration"], "READY")]
    tile_w = min(data["width"] * 10, 720)
    tile_h = data["height"] * 20 * tile_w // (data["width"] * 10)
    sheet = Image.new("RGB", (tile_w * 2 + 48, (tile_h + 42) * 4 + 16), "#101719")
    draw = ImageDraw.Draw(sheet)
    for i, (ms, label) in enumerate(steps):
        x, y = 16 + (i % 2) * (tile_w + 16), 16 + (i // 2) * (tile_h + 42)
        draw.text((x, y), f"{label}  {ms} ms", font=label_font, fill="#ffb86c")
        sheet.paste(render(ms).resize((tile_w, tile_h), Image.Resampling.LANCZOS), (x, y + 28))
    sheet.save(path.with_suffix(".png"))
    render(data["duration"]).save(path.with_name(path.stem + "-ready.png"))
    embedded = json.dumps(data, ensure_ascii=False).replace("<", "\\u003c")
    path.with_suffix(".html").write_text(HTML.replace("__DATA__", embedded).replace("__DURATION__", str(data["duration"])), encoding="utf-8")
    print(path.with_suffix(".html"))


if __name__ == "__main__":
    for name in ["desktop", "compact", "paper", "no-idle"]:
        export(ROOT / f"{name}.json")

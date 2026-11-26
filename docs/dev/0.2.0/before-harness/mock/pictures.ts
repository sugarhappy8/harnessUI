// Sample pictures for the mocked gallery: simple procedural "paintings" drawn on a canvas and
// returned as PNG (full size) or JPEG (thumbnail) bytes, as `studio_output_bytes` returns them.
// Each output id is seeded, so the same id always draws the same picture.

export type Scene = "coast" | "fox" | "teapot" | "city" | "waves" | "balloons" | "lake";

function rng(seedText: string): () => number {
  let h = 1779033703 ^ seedText.length;
  for (let i = 0; i < seedText.length; i++) {
    h = Math.imul(h ^ seedText.charCodeAt(i), 3432918353);
    h = (h << 13) | (h >>> 19);
  }
  let a = h >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

type Ctx = CanvasRenderingContext2D;

function vgrad(ctx: Ctx, y0: number, y1: number, stops: [number, string][]) {
  const g = ctx.createLinearGradient(0, y0, 0, y1);
  for (const [at, c] of stops) g.addColorStop(at, c);
  return g;
}

function glow(ctx: Ctx, x: number, y: number, r: number, color: string, alpha = 1) {
  const g = ctx.createRadialGradient(x, y, 0, x, y, r);
  g.addColorStop(0, color);
  g.addColorStop(1, "rgba(0,0,0,0)");
  ctx.save();
  ctx.globalAlpha = alpha;
  ctx.fillStyle = g;
  ctx.fillRect(x - r, y - r, r * 2, r * 2);
  ctx.restore();
}

function wash(ctx: Ctx, w: number, h: number, r: () => number, colors: string[], n = 160, alpha = 0.05) {
  ctx.save();
  for (let i = 0; i < n; i++) {
    ctx.globalAlpha = alpha * (0.4 + r());
    ctx.fillStyle = colors[Math.floor(r() * colors.length)]!;
    ctx.beginPath();
    ctx.ellipse(r() * w, r() * h, w * (0.02 + r() * 0.12), h * (0.01 + r() * 0.06), r() * Math.PI, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.restore();
}

function grain(ctx: Ctx, w: number, h: number, r: () => number, n = 2500, alpha = 0.05) {
  ctx.save();
  for (let i = 0; i < n; i++) {
    ctx.globalAlpha = alpha * r();
    ctx.fillStyle = r() > 0.5 ? "#ffffff" : "#000000";
    ctx.fillRect(r() * w, r() * h, 1 + r() * 1.5, 1 + r() * 1.5);
  }
  ctx.restore();
}

function coast(ctx: Ctx, w: number, h: number, r: () => number) {
  const shift = r();
  ctx.fillStyle = vgrad(ctx, 0, h * 0.62, [
    [0, shift > 0.5 ? "#2a1f55" : "#1f2a5c"],
    [0.55, shift > 0.5 ? "#b8507d" : "#a35a92"],
    [1, "#f7a95c"],
  ]);
  ctx.fillRect(0, 0, w, h);
  const sx = w * (0.25 + r() * 0.3);
  const sy = h * 0.57;
  glow(ctx, sx, sy, w * 0.35, "rgba(255,196,120,0.9)", 0.55);
  ctx.fillStyle = "#ffe2b0";
  ctx.beginPath();
  ctx.arc(sx, sy, w * 0.055, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = vgrad(ctx, h * 0.6, h, [
    [0, "#5a4a7c"],
    [1, "#1b1d3a"],
  ]);
  ctx.fillRect(0, h * 0.6, w, h * 0.4);
  ctx.save();
  for (let i = 0; i < 26; i++) {
    const y = h * (0.61 + r() * 0.33);
    ctx.globalAlpha = 0.25 + r() * 0.35;
    ctx.fillStyle = "#ffd59a";
    const len = w * (0.04 + r() * 0.12) * (1 - (y / h - 0.6));
    ctx.fillRect(sx - len / 2 + (r() - 0.5) * w * 0.08, y, len, 2 + r() * 3);
  }
  ctx.restore();
  // rocks
  ctx.fillStyle = "#17142a";
  ctx.beginPath();
  ctx.moveTo(w * 0.48, h);
  ctx.lineTo(w * 0.56, h * 0.74);
  ctx.lineTo(w * 0.63, h * 0.69);
  ctx.lineTo(w * 0.74, h * 0.66);
  ctx.lineTo(w * 0.86, h * 0.7);
  ctx.lineTo(w, h * 0.66);
  ctx.lineTo(w, h);
  ctx.closePath();
  ctx.fill();
  ctx.fillStyle = "#24203d";
  ctx.beginPath();
  ctx.moveTo(0, h);
  ctx.lineTo(0, h * 0.82);
  ctx.lineTo(w * 0.12, h * 0.86);
  ctx.lineTo(w * 0.22, h * 0.92);
  ctx.lineTo(w * 0.3, h);
  ctx.closePath();
  ctx.fill();
  // lighthouse
  const lx = w * 0.74;
  const base = h * 0.665;
  const top = h * 0.36;
  const bw = w * 0.05;
  const tw = w * 0.032;
  ctx.fillStyle = "#f1ebe1";
  ctx.beginPath();
  ctx.moveTo(lx - bw, base);
  ctx.lineTo(lx - tw, top);
  ctx.lineTo(lx + tw, top);
  ctx.lineTo(lx + bw, base);
  ctx.closePath();
  ctx.fill();
  ctx.fillStyle = "#b8322b";
  for (let i = 0; i < 3; i++) {
    const y0 = top + (base - top) * (0.18 + i * 0.28);
    const y1 = y0 + (base - top) * 0.11;
    const k0 = (y0 - top) / (base - top);
    const k1 = (y1 - top) / (base - top);
    ctx.beginPath();
    ctx.moveTo(lx - (tw + (bw - tw) * k0), y0);
    ctx.lineTo(lx + (tw + (bw - tw) * k0), y0);
    ctx.lineTo(lx + (tw + (bw - tw) * k1), y1);
    ctx.lineTo(lx - (tw + (bw - tw) * k1), y1);
    ctx.closePath();
    ctx.fill();
  }
  ctx.fillStyle = "#2b2236";
  ctx.fillRect(lx - tw * 1.35, top - h * 0.012, tw * 2.7, h * 0.014);
  ctx.fillStyle = "#ffe9a8";
  ctx.fillRect(lx - tw * 0.85, top - h * 0.055, tw * 1.7, h * 0.043);
  ctx.fillStyle = "#2b2236";
  ctx.beginPath();
  ctx.moveTo(lx - tw * 1.1, top - h * 0.055);
  ctx.lineTo(lx, top - h * 0.09);
  ctx.lineTo(lx + tw * 1.1, top - h * 0.055);
  ctx.closePath();
  ctx.fill();
  glow(ctx, lx, top - h * 0.033, w * 0.12, "rgba(255,236,170,0.9)", 0.8);
  ctx.save();
  ctx.globalAlpha = 0.16;
  ctx.fillStyle = "#fff3c4";
  const dir = r() > 0.5 ? -1 : 1;
  ctx.beginPath();
  ctx.moveTo(lx, top - h * 0.033);
  ctx.lineTo(lx + dir * w * 0.7, top - h * 0.16);
  ctx.lineTo(lx + dir * w * 0.7, top + h * 0.06);
  ctx.closePath();
  ctx.fill();
  ctx.restore();
  wash(ctx, w, h, r, ["#f7b267", "#f4845f", "#7b5ea7", "#4a6fa5", "#ffffff"], 220, 0.045);
  grain(ctx, w, h, r);
}

function fox(ctx: Ctx, w: number, h: number, r: () => number) {
  ctx.fillStyle = vgrad(ctx, 0, h, [
    [0, "#f4f7fc"],
    [1, "#d7e2f1"],
  ]);
  ctx.fillRect(0, 0, w, h);
  glow(ctx, w * 0.15, h * 0.1, w * 0.8, "rgba(255,226,186,0.9)", 0.55);
  ctx.save();
  ctx.globalAlpha = 0.35;
  ctx.fillStyle = "#b9c9e3";
  for (let i = 0; i < 7; i++) {
    ctx.beginPath();
    ctx.ellipse(r() * w, h * (0.55 + r() * 0.45), w * (0.15 + r() * 0.3), h * (0.03 + r() * 0.05), 0, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.restore();
  const cx = w * (0.47 + (r() - 0.5) * 0.08);
  const cy = h * 0.6;
  // shadow
  ctx.save();
  ctx.globalAlpha = 0.25;
  ctx.fillStyle = "#7d8fb3";
  ctx.beginPath();
  ctx.ellipse(cx + w * 0.03, cy + h * 0.12, w * 0.3, h * 0.05, 0, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
  // body
  const body = ctx.createRadialGradient(cx - w * 0.05, cy - h * 0.06, w * 0.02, cx, cy, w * 0.26);
  body.addColorStop(0, "#f08a3c");
  body.addColorStop(1, "#b9501c");
  ctx.fillStyle = body;
  ctx.beginPath();
  ctx.ellipse(cx, cy, w * 0.22, h * 0.13, -0.1, 0, Math.PI * 2);
  ctx.fill();
  // tail curling around
  ctx.fillStyle = "#d96a2b";
  ctx.beginPath();
  ctx.ellipse(cx + w * 0.05, cy + h * 0.08, w * 0.24, h * 0.07, 0.15, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = "#fbf6ef";
  ctx.beginPath();
  ctx.ellipse(cx - w * 0.16, cy + h * 0.075, w * 0.06, h * 0.045, 0.3, 0, Math.PI * 2);
  ctx.fill();
  // head
  const hx = cx - w * 0.15;
  const hy = cy - h * 0.03;
  ctx.fillStyle = "#e57b33";
  ctx.beginPath();
  ctx.ellipse(hx, hy, w * 0.085, h * 0.07, -0.2, 0, Math.PI * 2);
  ctx.fill();
  for (const dx of [-0.05, 0.02]) {
    ctx.fillStyle = "#c95a20";
    ctx.beginPath();
    ctx.moveTo(hx + w * dx, hy - h * 0.04);
    ctx.lineTo(hx + w * (dx + 0.02), hy - h * 0.13);
    ctx.lineTo(hx + w * (dx + 0.05), hy - h * 0.035);
    ctx.closePath();
    ctx.fill();
    ctx.fillStyle = "#3a2216";
    ctx.beginPath();
    ctx.moveTo(hx + w * (dx + 0.012), hy - h * 0.1);
    ctx.lineTo(hx + w * (dx + 0.02), hy - h * 0.13);
    ctx.lineTo(hx + w * (dx + 0.03), hy - h * 0.1);
    ctx.closePath();
    ctx.fill();
  }
  ctx.fillStyle = "#fbf6ef";
  ctx.beginPath();
  ctx.ellipse(hx - w * 0.045, hy + h * 0.025, w * 0.05, h * 0.03, -0.3, 0, Math.PI * 2);
  ctx.fill();
  ctx.strokeStyle = "#3a2216";
  ctx.lineWidth = w * 0.005;
  ctx.lineCap = "round";
  for (const dx of [-0.035, 0.015]) {
    ctx.beginPath();
    ctx.arc(hx + w * dx, hy - h * 0.005, w * 0.012, 0.15 * Math.PI, 0.85 * Math.PI);
    ctx.stroke();
  }
  ctx.fillStyle = "#2a1a12";
  ctx.beginPath();
  ctx.arc(hx - w * 0.088, hy + h * 0.02, w * 0.011, 0, Math.PI * 2);
  ctx.fill();
  // snow sparkle
  ctx.save();
  for (let i = 0; i < 140; i++) {
    ctx.globalAlpha = 0.4 + r() * 0.6;
    ctx.fillStyle = "#ffffff";
    ctx.beginPath();
    ctx.arc(r() * w, r() * h, 0.5 + r() * 2.2, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.restore();
  grain(ctx, w, h, r, 1500, 0.03);
}

function teapot(ctx: Ctx, w: number, h: number, r: () => number) {
  ctx.fillStyle = vgrad(ctx, 0, h, [
    [0, "#ece6dd"],
    [0.68, "#d6cdbf"],
    [0.7, "#e4ddd2"],
    [1, "#cfc5b6"],
  ]);
  ctx.fillRect(0, 0, w, h);
  glow(ctx, w * 0.3, h * 0.2, w * 0.7, "rgba(255,255,255,0.9)", 0.5);
  const cx = w * 0.5;
  const cy = h * 0.55;
  ctx.save();
  ctx.globalAlpha = 0.3;
  ctx.fillStyle = "#6f6455";
  ctx.beginPath();
  ctx.ellipse(cx, h * 0.76, w * 0.3, h * 0.04, 0, 0, Math.PI * 2);
  ctx.fill();
  ctx.restore();
  // foot / body
  const hue = r() > 0.5 ? ["#a9c7bb", "#6f988a"] : ["#b9c4d8", "#7686a6"];
  const bodyG = ctx.createLinearGradient(0, cy, 0, h * 0.76);
  bodyG.addColorStop(0, hue[0]!);
  bodyG.addColorStop(1, hue[1]!);
  ctx.fillStyle = bodyG;
  ctx.beginPath();
  ctx.moveTo(w * 0.2, h * 0.75);
  ctx.quadraticCurveTo(w * 0.18, h * 0.64, w * 0.3, h * 0.62);
  ctx.lineTo(w * 0.72, h * 0.6);
  ctx.quadraticCurveTo(w * 0.84, h * 0.62, w * 0.8, h * 0.75);
  ctx.closePath();
  ctx.fill();
  // spout (snail head)
  ctx.strokeStyle = hue[1]!;
  ctx.lineWidth = w * 0.045;
  ctx.lineCap = "round";
  ctx.beginPath();
  ctx.moveTo(w * 0.26, h * 0.66);
  ctx.quadraticCurveTo(w * 0.15, h * 0.6, w * 0.16, h * 0.46);
  ctx.stroke();
  ctx.strokeStyle = "#7b8f86";
  ctx.lineWidth = w * 0.006;
  for (const dx of [-0.012, 0.016]) {
    ctx.beginPath();
    ctx.moveTo(w * (0.16 + dx), h * 0.45);
    ctx.lineTo(w * (0.15 + dx * 2), h * 0.39);
    ctx.stroke();
    ctx.fillStyle = "#4a5a54";
    ctx.beginPath();
    ctx.arc(w * (0.15 + dx * 2), h * 0.385, w * 0.008, 0, Math.PI * 2);
    ctx.fill();
  }
  // shell
  const shell = ctx.createRadialGradient(cx + w * 0.02, cy - h * 0.1, w * 0.02, cx + w * 0.05, cy - h * 0.03, w * 0.24);
  shell.addColorStop(0, "#f3e3c8");
  shell.addColorStop(0.6, "#d8b48a");
  shell.addColorStop(1, "#a9825b");
  ctx.fillStyle = shell;
  ctx.beginPath();
  ctx.arc(cx + w * 0.05, cy - h * 0.06, w * 0.22, 0, Math.PI * 2);
  ctx.fill();
  ctx.strokeStyle = "rgba(120,84,50,0.75)";
  ctx.lineWidth = w * 0.01;
  ctx.beginPath();
  for (let t = 0; t < Math.PI * 6; t += 0.05) {
    const rr = w * 0.2 * (1 - t / (Math.PI * 6.4));
    const x = cx + w * 0.05 + Math.cos(t) * rr;
    const y = cy - h * 0.06 + Math.sin(t) * rr;
    if (t === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  }
  ctx.stroke();
  // lid knob and handle
  ctx.fillStyle = "#8a6a49";
  ctx.beginPath();
  ctx.arc(cx + w * 0.05, cy - h * 0.29, w * 0.022, 0, Math.PI * 2);
  ctx.fill();
  ctx.strokeStyle = hue[1]!;
  ctx.lineWidth = w * 0.03;
  ctx.beginPath();
  ctx.arc(w * 0.79, cy - h * 0.02, w * 0.07, -0.6 * Math.PI, 0.5 * Math.PI);
  ctx.stroke();
  glow(ctx, cx - w * 0.04, cy - h * 0.15, w * 0.08, "rgba(255,255,255,0.95)", 0.7);
  grain(ctx, w, h, r, 1200, 0.025);
}

function city(ctx: Ctx, w: number, h: number, r: () => number) {
  ctx.fillStyle = vgrad(ctx, 0, h, [
    [0, "#070a1c"],
    [0.6, "#1b1340"],
    [1, "#05060f"],
  ]);
  ctx.fillRect(0, 0, w, h);
  // buildings
  let x = -w * 0.05;
  while (x < w) {
    const bw = w * (0.08 + r() * 0.12);
    const bh = h * (0.3 + r() * 0.35);
    ctx.fillStyle = r() > 0.5 ? "#0e1128" : "#141737";
    ctx.fillRect(x, h * 0.66 - bh, bw, bh);
    for (let wy = h * 0.66 - bh + 10; wy < h * 0.64; wy += h * 0.035) {
      for (let wx = x + 6; wx < x + bw - 10; wx += w * 0.022) {
        if (r() > 0.62) {
          ctx.fillStyle = r() > 0.7 ? "rgba(127,211,255,0.75)" : "rgba(255,210,122,0.75)";
          ctx.fillRect(wx, wy, w * 0.01, h * 0.016);
        }
      }
    }
    x += bw + w * 0.01;
  }
  // neon signs
  const signs = [
    ["#ff3d8b", w * 0.14, h * 0.3, w * 0.05, h * 0.26],
    ["#2de2e6", w * 0.7, h * 0.36, w * 0.18, h * 0.06],
    ["#ffd23f", w * 0.44, h * 0.42, w * 0.12, h * 0.05],
  ] as const;
  for (const [c, sx, sy, sw, sh] of signs) {
    ctx.save();
    ctx.shadowColor = c;
    ctx.shadowBlur = w * 0.04;
    ctx.strokeStyle = c;
    ctx.lineWidth = w * 0.007;
    ctx.strokeRect(sx, sy, sw, sh);
    ctx.globalAlpha = 0.25;
    ctx.fillStyle = c;
    ctx.fillRect(sx, sy, sw, sh);
    ctx.restore();
    glow(ctx, sx + sw / 2, sy + sh / 2, w * 0.18, c, 0.18);
  }
  // wet street with reflections
  ctx.fillStyle = vgrad(ctx, h * 0.66, h, [
    [0, "#0d0f22"],
    [1, "#04050c"],
  ]);
  ctx.fillRect(0, h * 0.66, w, h * 0.34);
  ctx.save();
  for (const [c, sx, , sw] of signs) {
    const g = ctx.createLinearGradient(0, h * 0.66, 0, h);
    g.addColorStop(0, c);
    g.addColorStop(1, "rgba(0,0,0,0)");
    ctx.globalAlpha = 0.35;
    ctx.fillStyle = g;
    for (let i = 0; i < 6; i++) ctx.fillRect(sx + r() * sw, h * 0.66, w * 0.006 + r() * w * 0.01, h * 0.34 * (0.4 + r() * 0.6));
  }
  ctx.restore();
  // rain
  ctx.save();
  ctx.strokeStyle = "rgba(200,220,255,0.18)";
  ctx.lineWidth = 1.2;
  for (let i = 0; i < 260; i++) {
    const rx = r() * w;
    const ry = r() * h;
    ctx.beginPath();
    ctx.moveTo(rx, ry);
    ctx.lineTo(rx - w * 0.01, ry + h * 0.04);
    ctx.stroke();
  }
  ctx.restore();
  grain(ctx, w, h, r, 1800, 0.05);
}

function waves(ctx: Ctx, w: number, h: number, r: () => number) {
  ctx.fillStyle = vgrad(ctx, 0, h * 0.45, [
    [0, "#9fb7c9"],
    [1, "#dfe7ea"],
  ]);
  ctx.fillRect(0, 0, w, h * 0.45);
  ctx.fillStyle = vgrad(ctx, h * 0.45, h * 0.72, [
    [0, "#2f5d6e"],
    [1, "#1c3a47"],
  ]);
  ctx.fillRect(0, h * 0.45, w, h * 0.27);
  ctx.fillStyle = "#161616";
  ctx.fillRect(0, h * 0.72, w, h * 0.28);
  ctx.save();
  for (let i = 0; i < 9; i++) {
    const y = h * (0.5 + i * 0.03);
    ctx.globalAlpha = 0.5 + r() * 0.4;
    ctx.strokeStyle = "#f4f7f6";
    ctx.lineWidth = h * (0.006 + r() * 0.01);
    ctx.beginPath();
    for (let xx = 0; xx <= w; xx += w / 40) {
      const yy = y + Math.sin(xx / (w * 0.07) + i + r()) * h * 0.01;
      if (xx === 0) ctx.moveTo(xx, yy);
      else ctx.lineTo(xx, yy);
    }
    ctx.stroke();
  }
  ctx.globalAlpha = 0.85;
  ctx.fillStyle = "#eef3f2";
  ctx.beginPath();
  ctx.moveTo(0, h * 0.72);
  for (let xx = 0; xx <= w; xx += w / 30) ctx.lineTo(xx, h * (0.7 + r() * 0.05));
  ctx.lineTo(w, h * 0.78);
  ctx.lineTo(0, h * 0.78);
  ctx.closePath();
  ctx.fill();
  ctx.restore();
  grain(ctx, w, h, r, 2000, 0.06);
}

function balloons(ctx: Ctx, w: number, h: number, r: () => number) {
  ctx.fillStyle = vgrad(ctx, 0, h, [
    [0, "#5b7fbf"],
    [0.6, "#f2b38a"],
    [1, "#f6d29b"],
  ]);
  ctx.fillRect(0, 0, w, h);
  glow(ctx, w * 0.75, h * 0.8, w * 0.5, "rgba(255,220,160,0.9)", 0.6);
  ctx.fillStyle = "#3d4a3a";
  ctx.beginPath();
  ctx.moveTo(0, h);
  for (let xx = 0; xx <= w; xx += w / 12) ctx.lineTo(xx, h * (0.82 + r() * 0.06));
  ctx.lineTo(w, h);
  ctx.closePath();
  ctx.fill();
  const colors = ["#e63946", "#f4a261", "#2a9d8f", "#e9c46a", "#8338ec", "#ff006e"];
  for (let i = 0; i < 9; i++) {
    const bx = r() * w;
    const by = h * (0.15 + r() * 0.55);
    const br = w * (0.025 + r() * 0.06);
    const c1 = colors[i % colors.length]!;
    const c2 = colors[(i + 2) % colors.length]!;
    ctx.save();
    ctx.beginPath();
    ctx.ellipse(bx, by, br, br * 1.15, 0, 0, Math.PI * 2);
    ctx.clip();
    for (let s = -3; s <= 3; s++) {
      ctx.fillStyle = s % 2 === 0 ? c1 : c2;
      ctx.fillRect(bx + s * br * 0.3 - br * 0.15, by - br * 1.2, br * 0.3, br * 2.4);
    }
    ctx.restore();
    ctx.fillStyle = "#5a3d2b";
    ctx.fillRect(bx - br * 0.15, by + br * 1.35, br * 0.3, br * 0.25);
  }
  grain(ctx, w, h, r, 1500, 0.04);
}

function lake(ctx: Ctx, w: number, h: number, r: () => number) {
  ctx.fillStyle = vgrad(ctx, 0, h, [
    [0, "#f6c9a8"],
    [0.5, "#c9d6e3"],
    [1, "#6f8ca3"],
  ]);
  ctx.fillRect(0, 0, w, h);
  for (let i = 0; i < 3; i++) {
    ctx.fillStyle = ["#8a97b0", "#6b7a96", "#4a5a76"][i]!;
    ctx.beginPath();
    ctx.moveTo(0, h * 0.55);
    for (let xx = 0; xx <= w; xx += w / 8) ctx.lineTo(xx, h * (0.3 + i * 0.07 + r() * 0.12));
    ctx.lineTo(w, h * 0.55);
    ctx.closePath();
    ctx.fill();
  }
  wash(ctx, w, h, r, ["#ffffff", "#f6c9a8", "#c9d6e3"], 140, 0.06);
}

const SCENES: Record<Scene, (ctx: Ctx, w: number, h: number, r: () => number) => void> = {
  coast,
  fox,
  teapot,
  city,
  waves,
  balloons,
  lake,
};

const cache = new Map<string, Promise<ArrayBuffer>>();

/** The bytes of a sample picture: `scene` drawn for `id` at `w` × `h`, as PNG or JPEG. */
export function pictureBytes(id: string, scene: Scene, w: number, h: number, type: "image/png" | "image/jpeg") {
  const key = `${id}|${scene}|${w}x${h}|${type}`;
  let made = cache.get(key);
  if (!made) {
    made = new Promise<ArrayBuffer>((resolve, reject) => {
      const canvas = document.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      const ctx = canvas.getContext("2d");
      if (!ctx) return reject(new Error("no canvas"));
      // Drawn at a 1024 px design size, scaled to the size asked for.
      const scale = Math.min(w, h) / 1024;
      ctx.scale(scale, scale);
      SCENES[scene](ctx, w / scale, h / scale, rng(id));
      canvas.toBlob(
        (blob) => (blob ? blob.arrayBuffer().then(resolve, reject) : reject(new Error("no blob"))),
        type,
        0.9,
      );
    });
    cache.set(key, made);
  }
  return made;
}

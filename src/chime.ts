/* เสียงติ้งติ่งสั้น ๆ ตอนมีเวอร์ชันใหม่ — สังเคราะห์ด้วย Web Audio ไม่ต้องมีไฟล์เสียง
   WebView ไม่ยอมให้เล่นเสียงก่อนผู้ใช้แตะหน้าจอ: ถ้ายังเล่นไม่ได้ จะรอเล่นตอนคลิก / กดปุ่มครั้งถัดไป */

let ac: AudioContext | null = null;

const ding = (ctx: AudioContext) => {
  // E6 → A6 + เสียงแปดเสียงสูงเบา ๆ ให้ออกแนวกระดิ่ง
  [1318.5, 1760].forEach((f, i) => {
    const t = ctx.currentTime + i * 0.13;
    for (const [mul, vol] of [[1, 0.12], [2, 0.03]]) {
      const o = ctx.createOscillator();
      const g = ctx.createGain();
      o.type = "sine";
      o.frequency.value = f * mul;
      g.gain.setValueAtTime(0, t);
      g.gain.linearRampToValueAtTime(vol, t + 0.008);
      g.gain.exponentialRampToValueAtTime(0.0001, t + 0.55);
      o.connect(g).connect(ctx.destination);
      o.start(t);
      o.stop(t + 0.6);
    }
  });
};

export const chime = () => {
  try {
    ac ??= new AudioContext();
    const ctx = ac;
    if (ctx.state === "running") return ding(ctx);
    ctx.resume().catch(() => {});
    const later = () => {
      window.removeEventListener("pointerdown", later);
      window.removeEventListener("keydown", later);
      ctx.resume().then(() => ding(ctx)).catch(() => {});
    };
    // resume สำเร็จทันที (บางเครื่องไม่บล็อก) ก็เล่นเลย ไม่ต้องรอคลิก
    setTimeout(() => {
      if (ctx.state === "running") return ding(ctx);
      window.addEventListener("pointerdown", later);
      window.addEventListener("keydown", later);
    }, 50);
  } catch {
    /* ไม่มีเสียงก็ไม่เป็นไร การแจ้งเตือนบนจอยังขึ้นอยู่ */
  }
};

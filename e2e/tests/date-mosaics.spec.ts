import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { expect, seedTrips, test } from "../support/gallery";

// Year, month and day cards tile their files into a frame whose size
// gallery.json sets per level, so every card at a level is one size however
// few files its period has.
test("year, month and day cards fill a frame of the configured size", async ({ page, isolatedGallery: gallery }) => {
  seedTrips(gallery.libraryRoot);
  await writeFile(
    join(gallery.libraryRoot, ".videre", "gallery.json"),
    JSON.stringify({
      routes: {
        date: {
          year: { width: 320, height: 200 },
          month: { width: 240, height: 150 },
          day: { width: 180, height: 120 },
        },
      },
    })
  );

  for (const [route, w, h] of [
    ["/date", 320, 200],
    ["/date/2020", 240, 150],
    ["/date/2020/03", 180, 120],
  ] as const) {
    await page.goto(gallery.baseURL + route);
    const frames = page.locator(".date-mosaic");
    await expect(frames.first()).toBeVisible();
    const sizes = await frames.evaluateAll((els) =>
      els.map((el) => {
        const r = el.getBoundingClientRect();
        const covered = [...el.children].reduce((sum, c) => {
          const b = c.getBoundingClientRect();
          return sum + b.width * b.height;
        }, 0);
        return { w: Math.round(r.width), h: Math.round(r.height), filled: covered / (r.width * r.height) };
      })
    );
    for (const s of sizes) {
      expect(s.w, route).toBe(w);
      expect(s.h, route).toBe(h);
      expect(s.filled, route).toBeCloseTo(1, 2);
    }
  }
});

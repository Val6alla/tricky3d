import { describe, expect, it } from "vitest";
import { fitPreset, presetNames, presetsWith, presetsWithout } from "./presets";

describe("presets", () => {
  it("are kept by scope, then by name, and a save replaces one of the same name", () => {
    const one = presetsWith({}, "wall", "Thin", { threshold: 0.8 });
    const two = presetsWith(one, "wall", "Thick", { threshold: 3 });
    const three = presetsWith(two, "skin", "Thin", { upAngle: 30 });
    expect(presetNames(three, "wall")).toEqual(["Thick", "Thin"]);
    expect(presetNames(three, "skin")).toEqual(["Thin"]);
    expect(presetNames(null, "wall")).toEqual([]);
    expect(presetsWith(three, "wall", "Thin", { threshold: 1 }).wall.Thin).toEqual({ threshold: 1 });
    expect(presetsWithout(three, "wall", "Thin")).toEqual({ wall: { Thick: { threshold: 3 } }, skin: { Thin: { upAngle: 30 } } });
    expect(presetsWithout(three, "removability", "x").removability).toEqual({});
  });

  it("load only the values the dialog knows, of the right types", () => {
    const defaults = { threshold: 0.4, simple: false, colors: "red-green" };
    expect(fitPreset({ threshold: 2, simple: true, colors: "blue-yellow-gradient", extra: 1 }, defaults)).toEqual({
      threshold: 2,
      simple: true,
      colors: "blue-yellow-gradient",
    });
    expect(fitPreset({ threshold: "2", simple: 1, colors: null }, defaults)).toEqual({});
    expect(fitPreset({ threshold: Number.NaN }, defaults)).toEqual({});
  });
});

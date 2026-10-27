import { describe, expect, it } from "vitest";
import { parseNumber } from "./numbers";

describe("parseNumber", () => {
  it("parses typed numbers", () => {
    expect(parseNumber(" 12.5 ")).toBe(12.5);
    expect(parseNumber("12,5")).toBe(12.5);
    expect(parseNumber("-3 mm")).toBe(-3);
    expect(parseNumber(".5")).toBe(0.5);
    expect(parseNumber("")).toBeNull();
    expect(parseNumber("abc")).toBeNull();
    expect(parseNumber("1.2.3")).toBeNull();
    expect(parseNumber("Infinity")).toBeNull();
  });
});

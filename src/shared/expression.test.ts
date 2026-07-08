import { describe, expect, it } from "vitest";
import { calculateExpression } from "./expression";

describe("calculateExpression", () => {
  it("evaluates multiplication before addition", () => {
    expect(calculateExpression("7 + 5 * 2")).toBe("17");
  });

  it("supports decimal math", () => {
    expect(calculateExpression("10 / 4")).toBe("2.5");
  });

  it("supports modulo math", () => {
    expect(calculateExpression("10 % 4")).toBe("2");
  });

  it("rejects unknown tokens", () => {
    expect(calculateExpression("7 + nope")).toBe("Invalid expression");
  });
});

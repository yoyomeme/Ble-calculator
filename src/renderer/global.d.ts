import type { NativeCalculatorApi } from "../shared/calculator-api";

declare global {
  interface Window {
    calculator?: NativeCalculatorApi;
  }
}

export {};

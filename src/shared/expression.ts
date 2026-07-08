export function calculateExpression(expression: string): string {
  const sanitized = expression.replace(/\s+/g, "");
  if (!/^-?\d+(\.\d+)?([+\-*/%]-?\d+(\.\d+)?)*$/.test(sanitized)) {
    return "Invalid expression";
  }

  const tokens = sanitized.match(/-?\d+(?:\.\d+)?|[+\-*/%]/g);
  if (!tokens) {
    return "Invalid expression";
  }

  const values: number[] = [];
  const ops: string[] = [];
  const precedence: Record<string, number> = { "+": 1, "-": 1, "*": 2, "/": 2, "%": 2 };

  for (const token of tokens) {
    if (token in precedence) {
      const tokenPrecedence = precedence[token];
      if (tokenPrecedence === undefined) {
        return "Invalid expression";
      }

      while (ops.length > 0) {
        const op = ops[ops.length - 1];
        const opPrecedence = op === undefined ? undefined : precedence[op];
        if (opPrecedence === undefined || opPrecedence < tokenPrecedence) {
          break;
        }
        applyOp(values, ops.pop() ?? "");
      }
      ops.push(token);
    } else {
      values.push(Number(token));
    }
  }

  while (ops.length > 0) {
    applyOp(values, ops.pop() ?? "");
  }

  const result = values[0];
  if (result === undefined) {
    return "Invalid expression";
  }

  return Number.isFinite(result) ? String(Number(result.toFixed(8))) : "Invalid expression";
}

function applyOp(values: number[], op: string): void {
  const right = values.pop();
  const left = values.pop();

  if (left === undefined || right === undefined) {
    values.push(Number.NaN);
    return;
  }

  switch (op) {
    case "+":
      values.push(left + right);
      break;
    case "-":
      values.push(left - right);
      break;
    case "*":
      values.push(left * right);
      break;
    case "/":
      values.push(right === 0 ? Number.NaN : left / right);
      break;
    case "%":
      values.push(right === 0 ? Number.NaN : left % right);
      break;
    default:
      values.push(Number.NaN);
  }
}

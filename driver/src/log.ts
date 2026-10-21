/**
 * Logging, to stderr only.
 *
 * stdout carries the protocol and nothing else: a stray `console.log` in a driver module would
 * corrupt the stream Rust reads, and the failure would appear as a parse error in a command that
 * looked correct. Every write here goes through `process.stderr`.
 */
export interface Logger {
  info: (message: string, detail?: unknown) => void;
  warn: (message: string, detail?: unknown) => void;
  error: (message: string, detail?: unknown) => void;
}

export function createLogger(stream: NodeJS.WritableStream = process.stderr): Logger {
  const write = (level: string, message: string, detail?: unknown): void => {
    const tail = detail === undefined ? "" : ` ${describe(detail)}`;
    stream.write(`[${level}] ${message}${tail}\n`);
  };
  return {
    info: (message, detail) => {
      write("info", message, detail);
    },
    warn: (message, detail) => {
      write("warn", message, detail);
    },
    error: (message, detail) => {
      write("error", message, detail);
    },
  };
}

/** Renders a non-string detail without ever calling a `toString` that could print a secret. */
function describe(detail: unknown): string {
  if (detail instanceof Error) {
    return `${detail.name}: ${detail.message}`;
  }
  if (typeof detail === "string") {
    return detail;
  }
  try {
    return JSON.stringify(detail);
  } catch {
    return "<unprintable>";
  }
}

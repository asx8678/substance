import { createInterface } from "node:readline";

import { Blobs } from "./blobs.js";
import { DriverContext } from "./context.js";
import { createLogger } from "./log.js";
import type { Logger } from "./log.js";
import type { DriverCommand } from "./generated/DriverCommand.js";
import type { DriverEvent } from "./generated/DriverEvent.js";
import type { DriverResult } from "./generated/DriverResult.js";
import { redactSecrets } from "./redact.js";

/**
 * The driver's entry point: one JSON command per line in, one JSON event per line out.
 *
 * The loop is deliberately serial. A command is read, executed to completion and answered before
 * the next one is read, so an `act` cannot interleave with a `route`, and a record part cannot
 * be attributed to the wrong action. Rust's side (M3-T02) depends on that: it sends one command,
 * waits for `done`, and treats a second `done` as a protocol error.
 *
 * Nothing but protocol goes to stdout. Logs go to stderr, because a stray line on stdout is a
 * parse error in a command that looked correct.
 *
 * The contract Rust can rely on: **every input line ends with exactly one `done`**. A successful
 * command emits `record_part` or `result` first; a failed one emits `error` first. A line the
 * driver could not even parse still ends with `done`, so a client that waits per line is never
 * left waiting for a reply to a line that was rejected -- the mistake a first version of this
 * loop made.
 */

const log: Logger = createLogger();
const blobsRoot = process.env["SUBSTANCE_BLOB_DIR"];

/** Writes one event and resolves once the kernel took it. */
function emit(event: DriverEvent): Promise<void> {
  return new Promise((resolve) => {
    process.stdout.write(`${JSON.stringify(event)}\n`, () => {
      resolve();
    });
  });
}

/**
 * Stops when the reader goes away.
 *
 * Rust owns the other end of the pipe. If it exits first -- a cancelled run, a crashed host --
 * the next write raises `EPIPE`, and an unhandled one kills the driver with a stack trace for a
 * situation that has no remedy. Exiting is the whole correct answer: there is nobody to answer.
 */
function stopWhenReaderLeaves(): void {
  process.stdout.on("error", (error: NodeJS.ErrnoException) => {
    if (error.code === "EPIPE") {
      log.info("stdout closed; stopping");
      process.exit(0);
    }
    throw error;
  });
}

/** Writes a result, when a command produced one. */
function resultOf(subject: DriverResult): DriverEvent {
  return { event: "result", ...subject };
}

/**
 * Reads a command, rejecting anything that is not one.
 *
 * The check is by field: a command with no `cmd` string is not a command, and a command whose
 * `cmd` is unknown is not one the driver can run. Every field beyond that is read by the handler,
 * which is where the generated types are enforced by `tsc`; a runtime schema would be a second
 * copy of the protocol, and a copy that drifts is worse than a cast at one boundary.
 */
function parseCommand(line: string): DriverCommand {
  const parsed: unknown = JSON.parse(line);
  if (typeof parsed !== "object" || parsed === null) {
    throw new Error("a command must be a JSON object");
  }
  const cmd = (parsed as { cmd?: unknown }).cmd;
  if (typeof cmd !== "string") {
    throw new Error("a command must have a string \"cmd\"");
  }
  return parsed as DriverCommand;
}

/** Runs one command against the context, answering with its events. */
async function dispatch(
  command: DriverCommand,
  holder: { context: DriverContext | null },
): Promise<void> {
  if (command.cmd === "open_context") {
    const blobs = new Blobs(command.blob_dir === "" ? required(blobsRoot) : command.blob_dir);
    const context = new DriverContext(command, blobs, log);
    await context.start();
    holder.context = context;
    return;
  }

  if (command.cmd === "close_context") {
    await holder.context?.close();
    holder.context = null;
    return;
  }

  const context = holder.context;
  if (!context?.ready) {
    throw new Error(`${command.cmd} needs an open context`);
  }

  switch (command.cmd) {
    case "goto": {
      const record = await context.goto(command.url);
      await emit({ event: "record_part", ...record.part });
      return;
    }
    case "act": {
      const record = await context.act(command.kind, command.target, command.value);
      await emit({ event: "record_part", ...record.part });
      return;
    }
    case "list_controls": {
      const found = await context.listControls();
      await emit(resultOf({ kind: "controls", ...found }));
      return;
    }
    case "layout": {
      const measured = await context.layout();
      await emit(resultOf({ kind: "layout", ...measured }));
      return;
    }
    case "login": {
      const outcome = await context.login(command);
      await emit(resultOf({ kind: "login", ...outcome }));
      return;
    }
    case "snapshot_dom": {
      const snapshot = await context.snapshotDom();
      // A snapshot is a question about the page, not an effect, so it answers as a result and
      // never as a record part: a record part with an empty UI section would claim an action ran.
      await emit(resultOf({ kind: "snapshot_dom", ...snapshot }));
      return;
    }
    case "axe": {
      const violations = await context.axe();
      await emit(resultOf({ kind: "axe", screen: context.screen(), violations }));
      return;
    }
    case "route": {
      await context.route(command.pattern, command.mode, command.ms);
      return;
    }
    case "clock": {
      await context.clock(command.op, command.value);
      return;
    }
    case "trace": {
      await context.trace(command.op, command.path);
      log.info(`trace: ${command.op} ${command.path}`);
      return;
    }
    default: {
      // Exhaustive by construction: `command` is narrowed to `never` when every case is handled,
      // so a new variant in the generated types fails `tsc` here instead of at runtime.
      const unreachable: never = command;
      throw new Error(`unknown command ${JSON.stringify(unreachable)}`);
    }
  }
}

/** A value the environment must supply. */
function required(name: string | undefined): string {
  if (name === undefined || name === "") {
    throw new Error("open_context needs a blob_dir, or SUBSTANCE_BLOB_DIR in the environment");
  }
  return name;
}

/** The message a failure reports, with every session secret replaced. */
function safeMessage(error: unknown, secrets: ReadonlySet<string>): string {
  return redactSecrets(error instanceof Error ? error.message : String(error), secrets);
}

/** Reads commands until the input ends, answering each one. */
async function main(): Promise<void> {
  stopWhenReaderLeaves();
  const holder: { context: DriverContext | null } = { context: null };
  const lines = createInterface({ input: process.stdin, crlfDelay: Number.POSITIVE_INFINITY });

  await emit({ event: "ready" });

  for await (const line of lines) {
    const trimmed = line.trim();
    if (trimmed === "") {
      continue;
    }
    let command: DriverCommand;
    try {
      command = parseCommand(trimmed);
    } catch (error) {
      // No context to ask, so the command name is unknown by definition.
      await emit({
        event: "error",
        cmd: "<unparsed>",
        message: safeMessage(error, new Set<string>()),
        recoverable: true,
      });
      // The line is finished even though no command ran: the invariant above is per line, not
      // per command, so a client waiting for a reply is never stranded.
      await emit({ event: "done" });
      continue;
    }

    try {
      await dispatch(command, holder);
      await emit({ event: "done" });
    } catch (error) {
      // Redacted before anything reads it: `log.error` takes the raw error, and a Playwright
      // failure message can quote the selector it filled -- which, for a login, is not the
      // secret, but the value passed to it may appear in a form-value error.
      const secrets = new Set<string>(
        command.cmd === "login" ? [command.password_value] : [],
      );
      log.error(`${command.cmd} failed: ${safeMessage(error, secrets)}`);
      // A failed command leaves the loop running: the context is usually still usable, and the
      // recorder decides whether to restart the driver. `recoverable` says what the driver knows:
      // a failure that closed the context is not recoverable.
      const recoverable = holder.context?.ready === true;
      await emit({
        event: "error",
        cmd: command.cmd,
        message: safeMessage(error, secrets),
        recoverable,
      });
      await emit({ event: "done" });
    }
  }

  // A closed stdin is a shutdown request, not a failure: the caller finished its work.
  await holder.context?.close();
  await emit({ event: "done" });
  log.info(`driver stopped`);
}

await main();

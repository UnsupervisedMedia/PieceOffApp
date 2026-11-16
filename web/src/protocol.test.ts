// @vitest-environment node
import { readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it } from 'vitest';
import init, { message_types_json, protocol_version } from '../wasm/pieceoff_web_wasm.js';
import {
  CLIENT_MESSAGE_TYPES,
  PROTOCOL_VERSION,
  SERVER_MESSAGE_TYPES,
  type ClientMessage,
  type ServerMessage,
} from './protocol.gen';
import { checkServerProtocol } from './protocol';

// Compile-time: the generated tag lists and the generated unions name the
// same tags, both ways (scripts/web-check.sh typechecks this file).
type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;
const clientTagsMatch: Same<ClientMessage['type'], (typeof CLIENT_MESSAGE_TYPES)[number]> = true;
const serverTagsMatch: Same<ServerMessage['type'], (typeof SERVER_MESSAGE_TYPES)[number]> = true;

let rust: { client: string[]; server: string[] };
let v: number;

beforeAll(async () => {
  // wasm-bindgen's `--target web` glue fetches by URL; under node hand it the bytes.
  await init({
    module_or_path: readFileSync(new URL('../wasm/pieceoff_web_wasm_bg.wasm', import.meta.url)),
  });
  rust = JSON.parse(message_types_json());
  v = protocol_version();
});

describe('generated protocol types', () => {
  it('tie the tag lists to the unions', () => {
    expect(clientTagsMatch && serverTagsMatch).toBe(true);
  });

  it('have every ClientMessage tag the Rust core knows', () => {
    expect(rust.client.length).toBeGreaterThan(0);
    for (const tag of rust.client) expect(CLIENT_MESSAGE_TYPES).toContain(tag);
    expect([...CLIENT_MESSAGE_TYPES].sort()).toEqual([...rust.client].sort());
  });

  it('have every ServerMessage tag the Rust core knows', () => {
    expect(rust.server.length).toBeGreaterThan(0);
    for (const tag of rust.server) expect(SERVER_MESSAGE_TYPES).toContain(tag);
    expect([...SERVER_MESSAGE_TYPES].sort()).toEqual([...rust.server].sort());
  });

  it('describe the protocol version the core speaks', () => {
    expect(PROTOCOL_VERSION).toBe(v);
  });
});

describe('checkServerProtocol', () => {
  it('asks for a client update when the server is newer', () => {
    expect(checkServerProtocol(v + 1, v)).toBe('update-client');
  });

  it('accepts a matching server', () => {
    expect(checkServerProtocol(v, v)).toBe('ok');
  });

  it('blames the server when it is older', () => {
    expect(checkServerProtocol(v - 1, v)).toBe('update-server');
  });
});

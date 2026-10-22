import type { RoomDO } from "./room";

export interface Env {
  ROOMS: DurableObjectNamespace<RoomDO>;
  /** Longest a changed room waits before writing to storage (default 5000). */
  FLUSH_MS?: string;
  /** A joined socket silent this long is dropped (default 90000). */
  IDLE_SOCKET_MS?: string;
}

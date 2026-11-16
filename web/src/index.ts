// Entry for dist/play.js: the lobby app. The board (WebGL) is loaded later
// through app/loadBoard.ts and must never be imported from here.
import { boot } from './app/main';

export { boot, startApp } from './app/main';

// The page mounts the client by providing <div id="pieceoff-play"></div>.
if (typeof document !== 'undefined' && document.getElementById('pieceoff-play')) {
  void boot();
}

// screen.html = the receiver's own page, pointed at the app's entry script,
// so the app and the browser always show the same receiver.
import { readFileSync, writeFileSync } from "node:fs";

const page = readFileSync(new URL("../../receiver/index.html", import.meta.url), "utf8");
if (!page.includes('src="/src/main.ts"')) throw new Error("receiver/index.html changed: update screen-page.mjs");
writeFileSync(new URL("../screen.html", import.meta.url), page.replace('src="/src/main.ts"', 'src="/src/screen.ts"'));
console.log("screen.html written");

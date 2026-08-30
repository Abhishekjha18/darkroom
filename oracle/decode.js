// Dev-time oracle: decode a JPEG with jpeg-js and write raw RGB.
const fs = require('fs');
const jpeg = require('jpeg-js');
const raw = jpeg.decode(fs.readFileSync(process.argv[2]), { formatAsRGBA: false });
fs.writeFileSync(process.argv[3], Buffer.from(raw.data));
process.stderr.write(`${raw.width}x${raw.height}\n`);

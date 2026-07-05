const fs = require('fs');
const path = require('path');

function readString(buf, offset) {
    let start = offset.val;
    while (offset.val < buf.length && buf[offset.val] !== 0) {
        offset.val++;
    }
    const str = buf.toString('utf8', start, offset.val);
    offset.val++;
    return str;
}

function dumpVpk(filePath) {
    const buf = fs.readFileSync(filePath);
    const signature = buf.readUInt32LE(0);
    if (signature !== 0x55aa1234) {
        console.log("Not a VPK");
        return;
    }

    const version = buf.readUInt32LE(4);
    let treeSize, headerSize;

    if (version === 1) {
        treeSize = buf.readUInt32LE(8);
        headerSize = 12;
    } else if (version === 2) {
        treeSize = buf.readUInt32LE(8);
        headerSize = 28;
    } else {
        return;
    }

    const treeBuf = buf.slice(headerSize, headerSize + treeSize);
    let offset = { val: 0 };

    while (offset.val < treeBuf.length) {
        const ext = readString(treeBuf, offset);
        if (!ext) break;

        while (offset.val < treeBuf.length) {
            const pathStr = readString(treeBuf, offset);
            if (!pathStr) break;

            while (offset.val < treeBuf.length) {
                const filename = readString(treeBuf, offset);
                if (!filename) break;

                if (offset.val + 18 > treeBuf.length) break;
                
                const preloadBytes = treeBuf.readUInt16LE(offset.val + 4);
                offset.val += 18;
                
                if (preloadBytes > 0) {
                    offset.val += preloadBytes;
                }

                const normPath = pathStr.trim();
                let fullPath;
                if (!normPath) {
                    fullPath = `${filename}.${ext}`;
                } else {
                    fullPath = `${normPath}/${filename}.${ext}`;
                }

                if (fullPath.toLowerCase().includes("addonimage") || fullPath.toLowerCase().includes("addoninfo")) {
                    console.log(`${filePath}: ${fullPath} (pathStr: "${pathStr}")`);
                }
            }
        }
    }
}

const dir = 'example-addons';
for (const file of fs.readdirSync(dir)) {
    if (file.endsWith('.vpk')) {
        dumpVpk(path.join(dir, file));
    }
}

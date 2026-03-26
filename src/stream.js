// QOI delta decoder + WebGL/Canvas2D renderer (shared by index.html and raw_page.html)
//
// Wire format: each WebSocket binary message is prefixed with 1 byte:
//   0x00 = keyframe  (full QOI image)
//   0x01 = delta     (XOR'd QOI image — apply XOR to previous frame)

// --- WASM decoder loader ---
let wasmDecodeKey = null;
let wasmDecodeDelta = null;
let decoderName = 'JS';
let jsPrevPixels = null; // JS-fallback delta state

async function initWasmDecoder() {
    try {
        const { instance } = await WebAssembly.instantiateStreaming(fetch('/wasm/decoder.wasm'));
        const exports = instance.exports;
        const memory = exports.memory;

        function readResult() {
            const w = exports.get_width();
            const h = exports.get_height();
            const outPtr = exports.get_output_ptr();
            return { width: w, height: h, data: new Uint8ClampedArray(memory.buffer, outPtr, w * h * 4) };
        }

        wasmDecodeKey = function(buf) {
            const input = new Uint8Array(buf);
            const inputPtr = exports.alloc_input(input.byteLength);
            new Uint8Array(memory.buffer, inputPtr, input.byteLength).set(input);
            if (!exports.decode(input.byteLength)) return null;
            return readResult();
        };

        wasmDecodeDelta = function(buf) {
            const input = new Uint8Array(buf);
            const inputPtr = exports.alloc_input(input.byteLength);
            new Uint8Array(memory.buffer, inputPtr, input.byteLength).set(input);
            if (!exports.decode_delta(input.byteLength)) return null;
            return readResult();
        };

        decoderName = 'WASM';
    } catch (e) {
        console.warn('WASM decoder unavailable, using JS fallback:', e);
    }
}

function decodeFrame(buf) {
    const view = new Uint8Array(buf);
    if (view.length < 2) return null;

    const frameType = view[0];
    const qoiBuf = buf.slice(1);

    if (frameType === 0x00) {
        // Keyframe
        if (wasmDecodeKey) {
            return wasmDecodeKey(qoiBuf);
        } else {
            const frame = decodeQOI(qoiBuf);
            if (!frame) return null;
            jsPrevPixels = new Uint8ClampedArray(frame.data);
            return frame;
        }
    } else if (frameType === 0x01) {
        // Delta frame
        if (wasmDecodeDelta) {
            return wasmDecodeDelta(qoiBuf);
        } else {
            if (!jsPrevPixels) return null; // no keyframe yet — skip
            const frame = decodeQOI(qoiBuf);
            if (!frame) return null;
            for (let i = 0; i < frame.data.length; i++) {
                frame.data[i] ^= jsPrevPixels[i];
                jsPrevPixels[i] = frame.data[i];
            }
            return frame;
        }
    }
    return null;
}

// --- JS decoder (fallback) ---

function decodeQOI(buf) {
    const view = new DataView(buf);
    if (view.getUint32(0) !== 0x716F6966) return null;
    const w = view.getUint32(4);
    const h = view.getUint32(8);
    const pixels = new Uint8ClampedArray(w * h * 4);
    const index = new Uint8Array(64 * 4);
    let r = 0, g = 0, b = 0, a = 255;
    let pos = 14, pxPos = 0, end = buf.byteLength - 8;
    const data = new Uint8Array(buf);
    while (pos < end && pxPos < pixels.length) {
        const b1 = data[pos++];
        if (b1 === 0xFE) {
            r = data[pos++]; g = data[pos++]; b = data[pos++];
        } else if (b1 === 0xFF) {
            r = data[pos++]; g = data[pos++]; b = data[pos++]; a = data[pos++];
        } else {
            const op = b1 & 0xC0;
            if (op === 0x00) {
                const idx = (b1 & 0x3F) * 4;
                r = index[idx]; g = index[idx+1]; b = index[idx+2]; a = index[idx+3];
            } else if (op === 0x40) {
                r = (r + ((b1 >> 4) & 3) - 2) & 255;
                g = (g + ((b1 >> 2) & 3) - 2) & 255;
                b = (b + (b1 & 3) - 2) & 255;
            } else if (op === 0x80) {
                const b2 = data[pos++];
                const dg = (b1 & 0x3F) - 32;
                r = (r + dg - 8 + ((b2 >> 4) & 0x0F)) & 255;
                g = (g + dg) & 255;
                b = (b + dg - 8 + (b2 & 0x0F)) & 255;
            } else {
                let run = (b1 & 0x3F) + 1;
                while (run-- > 0 && pxPos < pixels.length) {
                    pixels[pxPos++] = r; pixels[pxPos++] = g;
                    pixels[pxPos++] = b; pixels[pxPos++] = a;
                }
                const idx2 = ((r * 3 + g * 5 + b * 7 + a * 11) & 63) * 4;
                index[idx2] = r; index[idx2+1] = g; index[idx2+2] = b; index[idx2+3] = a;
                continue;
            }
        }
        const idx = ((r * 3 + g * 5 + b * 7 + a * 11) & 63) * 4;
        index[idx] = r; index[idx+1] = g; index[idx+2] = b; index[idx+3] = a;
        pixels[pxPos++] = r; pixels[pxPos++] = g;
        pixels[pxPos++] = b; pixels[pxPos++] = a;
    }
    return { width: w, height: h, data: pixels };
}

function initRenderer(canvas) {
    const gl = canvas.getContext('webgl', { alpha: true, premultipliedAlpha: false });
    if (gl) {
        const vs = gl.createShader(gl.VERTEX_SHADER);
        gl.shaderSource(vs, 'attribute vec2 p;varying vec2 v;void main(){v=p*.5+.5;v.y=1.-v.y;gl_Position=vec4(p,0,1);}');
        gl.compileShader(vs);
        const fs = gl.createShader(gl.FRAGMENT_SHADER);
        gl.shaderSource(fs, 'precision mediump float;varying vec2 v;uniform sampler2D t;void main(){gl_FragColor=texture2D(t,v);}');
        gl.compileShader(fs);
        const prog = gl.createProgram();
        gl.attachShader(prog, vs); gl.attachShader(prog, fs);
        gl.linkProgram(prog); gl.useProgram(prog);
        const buf = gl.createBuffer();
        gl.bindBuffer(gl.ARRAY_BUFFER, buf);
        gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1,-1,1,-1,-1,1,1,1]), gl.STATIC_DRAW);
        const loc = gl.getAttribLocation(prog, 'p');
        gl.enableVertexAttribArray(loc);
        gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
        const tex = gl.createTexture();
        gl.bindTexture(gl.TEXTURE_2D, tex);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
        gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
        return {
            name: 'WebGL',
            draw(w, h, data) {
                if (canvas.width !== w || canvas.height !== h) {
                    canvas.width = w; canvas.height = h;
                    gl.viewport(0, 0, w, h);
                }
                gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, data);
                gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
            }
        };
    } else {
        const ctx = canvas.getContext('2d');
        return {
            name: 'Canvas2D',
            draw(w, h, data) {
                if (canvas.width !== w || canvas.height !== h) {
                    canvas.width = w; canvas.height = h;
                }
                ctx.putImageData(new ImageData(data, w, h), 0, 0);
            }
        };
    }
}

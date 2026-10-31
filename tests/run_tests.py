#!/usr/bin/env python3
"""End-to-end test of the Salvage engine.

Each scenario builds a real file system, writes and deletes files through the kernel
(or formats it), drives the engine over its protocol and compares recovered files
with the original SHA-256 values.

Usage: SALVAGE_SUDO=<password> python3 tests/run_tests.py [scenario ...]
The password is read from the environment only and is never written anywhere.
"""
import hashlib, json, os, shutil, subprocess, sys, tempfile, threading, time, zipfile, sqlite3, wave, struct

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ENGINE = os.environ.get("SALVAGE_ENGINE", os.path.join(ROOT, "engine/target/release/salvage-engine"))
WORK = os.environ.get("SALVAGE_WORK") or tempfile.mkdtemp(prefix="salvage-test-")
PW = os.environ.get("SALVAGE_SUDO", "")


def sudo(*cmd, check=True):
    r = subprocess.run(["sudo", "-S", "-p", "", *cmd], input=(PW + "\n").encode(),
                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if check and r.returncode != 0:
        raise RuntimeError(f"{cmd}: {r.stdout.decode()}")
    return r.stdout.decode()


def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def make_corpus(d):
    """Builds sample files in real formats. Accented names exercise Unicode handling."""
    os.makedirs(d, exist_ok=True)
    def path(*a):
        p = os.path.join(d, *a)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        return p
    def still(out, size, src="mandelbrot", extra=()):
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", f"{src}=size={size}", "-frames:v", "1", *extra, out], check=True)
    still(path("Photos/Zürich trip.jpg"), "1400x900", extra=("-q:v", "2"))
    still(path("Photos/écran principal.png"), "800x600", src="testsrc2")
    still(path("Photos/sub/ñandú.gif"), "120x120", src="testsrc")
    still(path("Photos/sub/holiday.webp"), "640x480", src="testsrc2")
    still(path("Photos/sub/égalité.bmp"), "320x240", src="testsrc")
    still(path("Photos/sub/scan.tif"), "320x240", src="testsrc2")
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=15:duration=3",
                    "-pix_fmt", "yuv420p", path("Videos/clip.mp4")], check=True)
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=160x120:rate=10:duration=2",
                    path("Videos/größe.mkv")], check=True)
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=4",
                    "-metadata", "title=Test", "-metadata", "artist=Salvage", path("Audio/été.mp3")], check=True)
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=220:duration=3", path("Audio/señal.ogg")], check=True)
    subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=330:duration=3", path("Audio/raw.flac")], check=True)
    with wave.open(path("Audio/note.wav"), "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(8000)
        w.writeframes(b"".join(struct.pack("<h", (i * 37) % 3000) for i in range(20000)))
    with open(path("Documents/report.pdf"), "wb") as f:
        body = b"BT /F1 24 Tf 40 700 Td (Salvage test) Tj ET\n" * 400
        f.write(b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n"
                b"3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R>>endobj\n"
                + b"4 0 obj<</Length %d>>stream\n" % len(body) + body + b"endstream endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n")
    with zipfile.ZipFile(path("Documents/März.zip"), "w", zipfile.ZIP_DEFLATED) as z:
        for i in range(30):
            z.writestr(f"file{i}.txt", ("line %d\n" % i) * 2000)
    with zipfile.ZipFile(path("Documents/offer.docx"), "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", "<Types/>")
        z.writestr("word/document.xml", "<w:document>" + "text " * 5000 + "</w:document>")
    db = sqlite3.connect(path("Documents/données.sqlite"))
    db.execute("create table t(a,b)")
    db.executemany("insert into t values(?,?)", [(i, "x" * 200) for i in range(3000)])
    db.commit(); db.close()
    with open(path("Documents/notes.txt"), "w") as f:
        f.write("accented note: çéñüöå\n" * 400)
    with open(path("Documents/mañana.txt"), "w") as f:
        f.write("very short\n")
    with open(path("Data/random_groß.bin"), "wb") as f:
        f.write(os.urandom(9 << 20))
    with open(path("Data/compressible.log"), "w") as f:
        for i in range(120000):
            f.write(f"{i:08d} repeated record line content\n")
    with open(path("Data/a very long file name à la carte that easily exceeds the short name limit and needs LFN.dat"), "wb") as f:
        f.write(os.urandom(70000))
    return {os.path.relpath(os.path.join(r, n), d): sha(os.path.join(r, n))
            for r, _, fs in os.walk(d) for n in fs}


class Engine:
    def __init__(self, source=None, session=None):
        args = [ENGINE, "serve"] + (["--source", source] if source else ["--session", session])
        self.p = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        self.n = 0
        self.replies = {}
        self.events = []
        self.cv = threading.Condition()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            m = json.loads(line)
            with self.cv:
                if "ev" in m:
                    self.events.append(m)
                else:
                    self.replies[m["id"]] = m
                self.cv.notify_all()

    def call(self, cmd, **kw):
        self.n += 1
        i = self.n
        self.p.stdin.write(json.dumps({"id": i, "cmd": cmd, **kw}) + "\n")
        self.p.stdin.flush()
        with self.cv:
            ok = self.cv.wait_for(lambda: i in self.replies, 60)
        assert ok, f"no reply: {cmd}"
        r = self.replies.pop(i)
        assert r["ok"], r
        return r["data"]

    def wait_event(self, name, timeout=300):
        with self.cv:
            ok = self.cv.wait_for(lambda: any(e["ev"] == name for e in self.events), timeout)
        assert ok, f"olay gelmedi: {name}"
        return [e for e in self.events if e["ev"] == name][-1]

    def scan(self, **opts):
        self.events.clear()
        self.call("scan", opts=opts)
        return self.wait_event("scan_done")

    def recover_all(self, dest, **opts):
        self.events.clear()
        self.call("recover", sel={"filter": {}}, opts={"dest": dest, **opts})
        return self.wait_event("recover_done")

    def close(self):
        try:
            self.p.stdin.close()
        except Exception:
            pass
        self.p.wait(10)


class Disk:
    def __init__(self, name, mb):
        self.img = os.path.join(WORK, name + ".img")
        with open(self.img, "wb") as f:
            f.truncate(mb << 20)
        self.mnt = os.path.join(WORK, "mnt-" + name)
        os.makedirs(self.mnt, exist_ok=True)
        self.loop = None

    def attach(self):
        self.loop = sudo("losetup", "-f", "--show", self.img).strip()

    def detach(self):
        sudo("losetup", "-d", self.loop)
        self.loop = None

    def clear_automounts(self):
        """Automounters such as udiskie can grab the loop device."""
        time.sleep(0.6)
        for t in subprocess.run(["findmnt", "-rn", "-S", self.loop, "-o", "TARGET"],
                                capture_output=True, text=True).stdout.split("\n"):
            if t and t != self.mnt:
                sudo("umount", t.replace("\\x20", " "), check=False)

    def mkfs(self, *cmd):
        self.attach()
        sudo(*cmd, self.loop)
        self.clear_automounts()

    def mount(self, *opts):
        sudo("mount", *opts, self.loop, self.mnt)
        sudo("chown", f"{os.getuid()}:{os.getgid()}", self.mnt, check=False)

    def umount(self):
        sudo("sync")
        sudo("umount", self.mnt)
        self.clear_automounts()

    def write(self, src, rel_files=None):
        for rel in rel_files:
            d = os.path.join(self.mnt, rel)
            sudo("mkdir", "-p", os.path.dirname(d))
            sudo("cp", os.path.join(src, rel), d)
        sudo("sync")


CARVABLE = (".jpg", ".png", ".gif", ".webp", ".bmp", ".tif", ".mp4", ".mkv", ".mp3", ".ogg",
            ".flac", ".wav", ".pdf", ".zip", ".docx", ".sqlite")


def outcomes(rec_dir, expected):
    got = {}
    for r, _, fs in os.walk(rec_dir):
        for n in fs:
            if n == "recovery-report.csv":
                continue
            got.setdefault(sha(os.path.join(r, n)), []).append(os.path.join(r, n))
    ok = [k for k, v in expected.items() if v in got]
    bad = [k for k, v in expected.items() if v not in got]
    return ok, bad, got


def named_ok(rec_dir, expected, got):
    """Were the name and folder kept too?"""
    n = 0
    for rel, h in expected.items():
        base = os.path.basename(rel)
        for p in got.get(h, []):
            if os.path.basename(p) == base and os.path.dirname(rel).split("/")[-1] in p:
                n += 1
                break
    return n


RESULTS = []


KNOWN_LIMITS = {"ext2"}


def report(name, mode, expected, rec_dir, ev):
    ok, bad, got = outcomes(rec_dir, expected)
    named = named_ok(rec_dir, expected, got)
    status = "PASS" if not bad else ("LIMIT" if name in KNOWN_LIMITS else "MISSING")
    RESULTS.append((name, mode, len(ok), len(expected), named, ev.get("found"), status))
    print(f"[{status}] {name:<22} {mode:<6} content {len(ok)}/{len(expected)}  name+folder {named}/{len(expected)}  found {ev.get('found')}  {ev.get('seconds', 0):.2f} s")
    for b in bad[:12]:
        print("    lost:", b)
    return bad


def scenario(name, mkfs, corpus, hashes, mount_opts=(), keep=None, size=256, modes=("quick",), mutate=None, scan_opts=None, carve_only=False, max_size=None, min_size=0):
    print(f"\n=== {name}")
    d = Disk(name, size)
    try:
        d.mkfs(*mkfs)
        d.mount(*mount_opts)
        d.write(corpus, list(hashes))
        # Delete half and keep half: deleted files must be found, kept ones must not be listed.
        rels = sorted(hashes)
        delete = rels[::2] if keep is None else [r for r in rels if r not in keep]
        for r in delete:
            sudo("rm", "-f", os.path.join(d.mnt, r))
        sudo("sync")
        d.umount()
        if mutate:
            mutate(d)
            d.clear_automounts()
        d.detach()
        expected = {r: hashes[r] for r in delete}
        if carve_only:
            # No metadata left: only formats with a signature are expected.
            expected = {r: v for r, v in expected.items() if r.endswith(CARVABLE)}
        if max_size or min_size:
            expected = {r: v for r, v in expected.items()
                        if min_size <= os.path.getsize(os.path.join(corpus, r)) <= (max_size or 1 << 62)}
        all_bad = []
        for mode in modes:
            e = Engine(source=d.img)
            ev = e.scan(mode=mode, **(scan_opts or {}))
            q = e.call("query", filter={}, sort={"by": "name"}, limit=2000)
            if ev.get("error"):
                print("ERROR", ev)
            logs = [x["msg"] for x in e.events if x["ev"] == "log"]
            for m in logs:
                print("   log:", m)
            rec = os.path.join(WORK, f"rec-{name}-{mode}")
            shutil.rmtree(rec, ignore_errors=True)
            r = e.recover_all(rec, keep_tree=True)
            all_bad += report(name, mode, expected, rec, ev)
            assert r.get("failed") == 0, r
            e.close()
            shutil.rmtree(rec, ignore_errors=True)
        return all_bad
    finally:
        if d.loop:
            sudo("umount", d.mnt, check=False)
            sudo("losetup", "-d", d.loop, check=False)
        if not os.environ.get("SALVAGE_KEEP"):
            os.remove(d.img)


def windows_delete(d, names):
    """Clears only the MFT in-use flag, as Windows does; $FILE_NAME stays."""
    def utf16(b):
        return b.decode("utf-16-le", "replace")
    with open(d.img, "r+b") as f:
        boot = f.read(512)
        bps = struct.unpack_from("<H", boot, 11)[0]
        cl = bps * boot[13]
        crec = struct.unpack_from("<b", boot, 0x40)[0]
        rs = (1 << -crec) if crec < 0 else crec * cl
        base = struct.unpack_from("<Q", boot, 0x30)[0] * cl
        hit = 0
        for no in range(24, 4096):
            f.seek(base + no * rs)
            r = f.read(rs)
            if r[:4] != b"FILE":
                continue
            p = struct.unpack_from("<H", r, 20)[0]
            while p + 8 < rs and struct.unpack_from("<I", r, p)[0] != 0xFFFFFFFF:
                ty, ln = struct.unpack_from("<II", r, p)
                if ln == 0:
                    break
                if ty == 0x30:
                    vo = struct.unpack_from("<H", r, p + 20)[0]
                    v = r[p + vo:]
                    n = v[64]
                    if utf16(v[66:66 + n * 2]) in names:
                        f.seek(base + no * rs + 22)
                        f.write(struct.pack("<H", struct.unpack_from("<H", r, 22)[0] & ~1))
                        hit += 1
                        break
                p += ln
    print(f"   Windows-style delete: {hit} records")


def fmt_mutate(*cmd):
    def f(d):
        # An automounter can grab the device again: retry at short intervals.
        for _ in range(10):
            d.clear_automounts()
            try:
                sudo(*cmd, d.loop)
                return
            except RuntimeError:
                time.sleep(0.5)
        sudo(*cmd, d.loop)
    return f


SCENARIOS = {}


def reg(f):
    SCENARIOS[f.__name__] = f
    return f


@reg
def fat32(c, h):
    scenario("fat32", ["mkfs.vfat", "-F", "32", "-n", "SALVAGETEST"], c, h, ("-o", "utf8"), modes=("quick", "deep"))


@reg
def fat16(c, h):
    scenario("fat16", ["mkfs.vfat", "-F", "16"], c, h, ("-o", "utf8"), size=128, modes=("quick",))


@reg
def exfat(c, h):
    scenario("exfat", ["mkfs.exfat", "-L", "SALVAGEEX"], c, h, modes=("quick", "deep"))


@reg
def ntfs(c, h):
    scenario("ntfs", ["mkfs.ntfs", "-Q", "-L", "SALVAGENT"], c, h, ("-t", "ntfs3"), modes=("quick", "deep"))


@reg
def ntfs_windows(c, h):
    # Files are deleted through the MFT flag alone, as on Windows, not with rm.
    rels = sorted(h)
    victims = rels[::2]
    print("\n=== ntfs-windows")
    d = Disk("ntfs-windows", 256)
    try:
        d.mkfs("mkfs.ntfs", "-Q", "-L", "WIN")
        d.mount("-t", "ntfs3")
        d.write(c, rels)
        d.umount()
        d.detach()
        windows_delete(d, {os.path.basename(r) for r in victims})
        expected = {r: h[r] for r in victims}
        e = Engine(source=d.img)
        ev = e.scan(mode="quick")
        rec = os.path.join(WORK, "rec-ntfs-windows")
        shutil.rmtree(rec, ignore_errors=True)
        e.recover_all(rec)
        report("ntfs-windows", "quick", expected, rec, ev)
        e.close()
    finally:
        if d.loop:
            sudo("losetup", "-d", d.loop, check=False)
        if not os.environ.get("SALVAGE_KEEP"):
            os.remove(d.img)


@reg
def ext4(c, h):
    scenario("ext4", ["mkfs.ext4", "-q", "-L", "SALVAGEE4"], c, h, modes=("quick", "deep"))


@reg
def ext3(c, h):
    scenario("ext3", ["mkfs.ext3", "-q"], c, h, modes=("quick",))


@reg
def ext2(c, h):
    # ext2 has no journal and the kernel erases block pointers: signature search is the only way.
    # This kernel also fragments ext2 files: a known limit.
    scenario("ext2", ["mkfs.ext2", "-q"], c, h, modes=("deep",), carve_only=True, max_size=12 << 10)


@reg
def btrfs_plain(c, h):
    scenario("btrfs-duz", ["mkfs.btrfs", "-q", "-f", "-L", "SALVAGEBT"], c, h, ("-o", "nodiscard,compress=no"), size=512, modes=("quick",))


@reg
def btrfs_zstd(c, h):
    scenario("btrfs-zstd", ["mkfs.btrfs", "-q", "-f"], c, h, ("-o", "nodiscard,compress-force=zstd"), size=512, modes=("quick",))


@reg
def btrfs_lzo(c, h):
    scenario("btrfs-lzo", ["mkfs.btrfs", "-q", "-f"], c, h, ("-o", "nodiscard,compress-force=lzo"), size=512, modes=("quick",))


@reg
def btrfs_zlib(c, h):
    scenario("btrfs-zlib", ["mkfs.btrfs", "-q", "-f"], c, h, ("-o", "nodiscard,compress-force=zlib"), size=512, modes=("quick",))


@reg
def xfs(c, h):
    # Current kernels zero the extents of a deleted inode: free space signature search.
    scenario("xfs", ["mkfs.xfs", "-q", "-f", "-L", "SALVAGEXF"], c, h, size=512, modes=("deep",), carve_only=True)


@reg
def f2fs(c, h):
    scenario("f2fs", ["mkfs.f2fs", "-q", "-f", "-l", "SALVAGEF2"], c, h, ("-o", "nodiscard"), size=512, modes=("quick", "deep"))


@reg
def fat_format(c, h):
    # Everything is deleted, then quick formatted: signature search must recover it.
    scenario("fat32-bicim", ["mkfs.vfat", "-F", "32"], c, h, ("-o", "utf8"), keep=set(),
             mutate=fmt_mutate("mkfs.vfat", "-F", "32"), modes=("deep",), carve_only=True)


@reg
def ntfs_format(c, h):
    scenario("ntfs-bicim", ["mkfs.ntfs", "-Q"], c, h, ("-t", "ntfs3"), keep=set(),
             mutate=fmt_mutate("mkfs.ntfs", "-Q", "-F"), modes=("full",), carve_only=True, min_size=1024)


def main():
    corpus = os.path.join(WORK, "corpus")
    shutil.rmtree(corpus, ignore_errors=True)
    hashes = make_corpus(corpus)
    total = sum(os.path.getsize(os.path.join(corpus, k)) for k in hashes)
    print(f"Sample data: {len(hashes)} files, {total >> 20} MiB; work folder {WORK}")
    names = sys.argv[1:] or list(SCENARIOS)
    failures = 0
    for n in names:
        try:
            SCENARIOS[n](corpus, hashes)
        except Exception as ex:
            failures += 1
            print(f"[ERROR] {n}: {ex}")
    print("\n" + "=" * 78)
    for r in RESULTS:
        print(f"{r[6]:<6} {r[0]:<16} {r[1]:<6} content {r[2]:>2}/{r[3]:<2}  name+folder {r[4]:>2}/{r[3]:<2}  found {r[5]}")
    bad = sum(1 for r in RESULTS if r[6] == "MISSING") + failures
    print(f"Result: {len(RESULTS) - bad + failures}/{len(RESULTS) + failures} scenarios complete")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())

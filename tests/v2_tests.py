#!/usr/bin/env python3
"""V2 feature tests: bad sectors, fragmented JPEG, LUKS, LVM, TRIM protection,
resume, emergency rescue, duplicates, verification, content search, export."""
import hashlib, json, os, random, shutil, subprocess, sys, time
sys.path.insert(0, os.path.dirname(__file__))
import run_tests as T
from run_tests import Engine, Disk, sudo, sha, WORK
R = {}
def ok(name, cond, info=""):
    R[name] = bool(cond); print(("[PASS] " if cond else "[FAIL] ") + name, info)
def jpg(path, size="1280x960", src="mandelbrot"):
    subprocess.run(["ffmpeg","-v","error","-y","-f","lavfi","-i",f"{src}=size={size}","-frames:v","1","-q:v","3",path],check=True)

def t_fragment():
    """A JPEG is split in two on 4K blocks, with random or text blocks in between."""
    good = 0; total = 0
    for seed, filler in [(1,"random"),(2,"text"),(3,"random"),(4,"pdf")]:
        random.seed(seed)
        p = f"{WORK}/frag{seed}.jpg"; jpg(p, "1600x1200", "mandelbrot" if seed%2 else "testsrc2")
        b = open(p,"rb").read(); h = hashlib.sha256(b).hexdigest()
        cut = (len(b)//2)//4096*4096
        n = 37
        if filler == "random": gap = random.randbytes(n*4096)
        elif filler == "text": gap = (b"lorem ipsum dolor sit amet\n"*8000)[:n*4096]
        else: gap = (b"%PDF-1.4 junk "+random.randbytes(1000))*3; gap = gap.ljust(n*4096, b"\0")
        img = f"{WORK}/frag{seed}.img"
        with open(img,"wb") as f:
            f.write(b"\0"*(1<<20)); f.write(b[:cut]); f.write(gap); f.write(b[cut:]); f.write(b"\0"*(1<<20))
        e = Engine(source=img); ev = e.scan(mode="full")
        out = f"{WORK}/fragout{seed}"; shutil.rmtree(out,ignore_errors=True); e.recover_all(out, keep_tree=False); e.close()
        hs = {sha(os.path.join(r,x)) for r,_,fs in os.walk(out) for x in fs if not x.endswith(".csv")}
        total += 1; good += h in hs
        print(f"   {filler}: {'joined' if h in hs else 'not joined'} ({len(b)} bytes)")
        shutil.rmtree(out,ignore_errors=True); os.remove(img)
    ok("fragmented JPEG join", good == total, f"{good}/{total}")

def t_bad_sectors():
    """A device with truly unreadable sectors, using dm-error."""
    d = Disk("bad", 64); d.attach()
    blob = os.urandom(32<<20)
    with open(d.img,"r+b") as f: f.write(blob)
    sudo("sync")
    sz = 64*2048; table = f"0 20000 linear {d.loop} 0\n20000 400 error\n20400 {sz-20400} linear {d.loop} 20400\n"
    tf=f"{WORK}/dm-table"; open(tf,"w").write(table)
    sudo("sh","-c",f"dmsetup create salvage-bad < {tf}")
    dev="/dev/mapper/salvage-bad"
    try:
        p = subprocess.Popen(["sudo","-S","-p","",T.ENGINE,"serve","--source",dev],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
        p.stdin.write(T.PW+"\n"); p.stdin.flush()
        evs=[]
        import threading
        threading.Thread(target=lambda:[evs.append(json.loads(l)) for l in p.stdout],daemon=True).start()
        while not any(x.get("ev")=="ready" for x in evs): time.sleep(.02)
        dest=f"{WORK}/bad.out"
        for f in (dest,dest+".salvage-map",dest+".bad-regions"):
            if os.path.exists(f): os.remove(f)
        p.stdin.write(json.dumps({"id":1,"cmd":"image","dest":dest})+"\n"); p.stdin.flush()
        t0=time.time()
        while not any(x.get("ev")=="image_done" for x in evs) and time.time()-t0<60: time.sleep(.05)
        r=[x for x in evs if x.get("ev")=="image_done"][0]
        got=open(dest,"rb").read()
        s=20000*512; e_=20400*512
        same_before=got[:s]==blob[:s]; same_after=got[e_:32<<20]==blob[e_:]; zero=got[s:e_]==b"\0"*(e_-s)
        ok("imaging with bad sectors", same_before and same_after and zero and r["bad_bytes"]==e_-s, f"bad {r['bad_bytes']} bytes, {r['seconds']:.1f} s")
        p.stdin.close(); p.wait(5)
    finally:
        sudo("dmsetup","remove","salvage-bad",check=False); d.detach(); os.remove(d.img)

def t_image_resume():
    img=f"{WORK}/res.img"
    with open(img,"wb") as f: f.write(os.urandom(600<<20))
    dest=f"{WORK}/res.out"
    for f in (dest,dest+".salvage-map"):
        if os.path.exists(f): os.remove(f)
    e=Engine(source=img); e.call("image",dest=dest); time.sleep(.06); e.call("cancel_task"); r=e.wait_event("image_done"); e.events.clear(); time.sleep(.1)
    partial=r["bytes"]
    e.call("image",dest=dest); r2=e.wait_event("image_done"); e.close()
    ok("imaging resume", not r2.get("cancelled") and sha(dest)==sha(img) and 0<partial<600<<20, f"resumed after {partial>>20} MiB")
    os.remove(img); os.remove(dest)

def t_luks_lvm(corpus,h):
    rels=sorted(h)[:6]
    # LUKS
    d=Disk("luks",128); d.attach()
    def s(*c,inp=None): return subprocess.run(["sudo","-S","-p","",*c],input=(T.PW+"\n"+(inp or "")).encode(),capture_output=True)
    s("cryptsetup","luksFormat","--batch-mode","--pbkdf","pbkdf2","--pbkdf-force-iterations","1000",d.loop,"-",inp="secret")
    s("cryptsetup","open",d.loop,"salvage-luks","--key-file","-",inp="secret")
    dm="/dev/mapper/salvage-luks"
    try:
        s("mkfs.ext4","-q",dm); time.sleep(.7)
        lst=T.subprocess.run([T.ENGINE,"devices"],capture_output=True,text=True).stdout
        ok("LUKS appears in the list", "salvage-luks" in lst)
        for t in subprocess.run(["findmnt","-rn","-S",dm,"-o","TARGET"],capture_output=True,text=True).stdout.split("\n"):
            if t: sudo("umount",t,check=False)
        sudo("mount",dm,d.mnt)
        for r in rels:
            sudo("mkdir","-p",os.path.dirname(f"{d.mnt}/{r}")); sudo("cp",f"{corpus}/{r}",f"{d.mnt}/{r}")
        sudo("sync"); 
        for r in rels: sudo("rm",f"{d.mnt}/{r}")
        sudo("sync"); sudo("umount",d.mnt); time.sleep(.3)
        for t in subprocess.run(["findmnt","-rn","-S",dm,"-o","TARGET"],capture_output=True,text=True).stdout.split("\n"):
            if t: sudo("umount",t,check=False)
        rec=_scan_dev(dm)
        got={sha(os.path.join(r,x)) for r,_,fs in os.walk(rec) for x in fs if not x.endswith(".csv")}
        n=sum(h[r] in got for r in rels)
        ok("recovery from inside LUKS", n==len(rels), f"{n}/{len(rels)}")
        shutil.rmtree(rec,ignore_errors=True)
    finally:
        s("cryptsetup","close","salvage-luks"); d.detach(); os.remove(d.img)

def _scan_dev(dev):
    import threading
    p=subprocess.Popen(["sudo","-S","-p","",T.ENGINE,"serve","--source",dev],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
    p.stdin.write(T.PW+"\n"); p.stdin.flush(); evs={}
    def rd():
        for l in p.stdout:
            m=json.loads(l)
            if "ev" in m: evs[m["ev"]]=m
    threading.Thread(target=rd,daemon=True).start()
    while "ready" not in evs: time.sleep(.02)
    p.stdin.write(json.dumps({"id":1,"cmd":"scan","opts":{"mode":"quick"}})+"\n"); p.stdin.flush()
    while "scan_done" not in evs: time.sleep(.02)
    rec=f"{WORK}/rec-dev"; shutil.rmtree(rec,ignore_errors=True)
    p.stdin.write(json.dumps({"id":2,"cmd":"recover","sel":{"filter":{}},"opts":{"dest":rec}})+"\n"); p.stdin.flush()
    while "recover_done" not in evs: time.sleep(.02)
    p.stdin.close(); p.wait(5)
    return rec

def t_scan_features(corpus,h):
    d=Disk("feat",256); d.mkfs("mkfs.vfat","-F","32"); d.mount("-o","utf8")
    rels=sorted(h)
    d.write(corpus,rels)
    # a copy with the same content
    a=[r for r in rels if r.endswith(".png")][0]; sudo("cp",f"{d.mnt}/{a}",f"{d.mnt}/copy.png"); sudo("sync")
    for r in rels: sudo("rm",f"{d.mnt}/{r}")
    sudo("rm",f"{d.mnt}/copy.png"); sudo("sync"); d.umount(); d.detach()
    # duplicate detection
    e=Engine(source=d.img); ev=e.scan(mode="quick")
    q=e.call("query",filter={},sort={"by":"name"},limit=2000)
    print("   ",q["total"],sorted((i["size"],i["name"][:14],i["chance"],i["path"][:12]) for i in q["items"] if i["size"]==os.path.getsize(f"{corpus}/{a}")))
    dups=[i for i in q["items"] if i.get("dup")]
    ok("duplicate detection", len(dups)==1, str([i["name"] for i in dups]))
    q2=e.call("query",filter={"hide_dupes":True},sort={"by":"name"},limit=5)
    ok("hide duplicates", q2["total"]==q["total"]-1)
    # verification
    e.events.clear(); e.call("validate",sel={"filter":{}}); r=e.wait_event("task_done")
    print("   corrupt:",[ (i["name"],i["size"]) for i in e.call("query",filter={},sort={"by":"name"},limit=2000)["items"] if i.get("note")=="note.corrupt"])
    ok("open-and-check verification", r["good"]>=8 and r["bad"]==0, f"good {r['good']} corrupt {r['bad']} unknown {r['unknown']}")
    # content search
    e.events.clear(); e.call("grep",text="ACCENTED NOTE".lower()); e.wait_event("task_done")
    q3=e.call("query",filter={"grep_only":True},sort={"by":"name"},limit=5)
    ok("content search", any("notes" in i["name"] for i in q3["items"]), str([i["name"] for i in q3["items"]]))
    # export
    for fmt in ("csv","json","html"):
        p=f"{WORK}/liste.{fmt}"; x=e.call("export",filter={},path=p,format=fmt)
        ok(f"export {fmt}", os.path.getsize(p)>100 and x["rows"]==q["total"])
    # recovery while scanning
    e.close()
    e=Engine(source=d.img); e.events.clear(); e.call("scan",opts={"mode":"full","max_rate":120<<20})
    time.sleep(1.2)
    rec=f"{WORK}/ayni-anda"; shutil.rmtree(rec,ignore_errors=True)
    ids=e.call("query",filter={},sort={"by":"size","desc":True},limit=3)["items"]
    e.call("recover",sel={"ids":[i["id"] for i in ids]},opts={"dest":rec})
    r=e.wait_event("recover_done"); running=not any(x["ev"]=="scan_done" for x in e.events)
    ok("recovery while scanning", r["ok"]==len(ids) and running and len(ids)>0, f"{r['ok']} files")
    # stop and resume
    e.call("cancel"); d1=e.wait_event("scan_done"); ok("stopped scan can be resumed", d1.get("resumable"))
    ses=f"{WORK}/k.salvage"; e.call("save_session",path=ses); before=d1["found"]; e.close()
    e=Engine(session=ses); e.events.clear(); e.call("scan",opts={"mode":"full","resume":True,"step":512}); d2=e.wait_event("scan_done")
    ok("scan resume", not d2.get("resumable") and d2["found"]>=before, f"{before} -> {d2['found']}"); e.close()
    # speed limit
    t0=time.time(); e=Engine(source=d.img); ev=e.scan(mode="full",max_rate=128<<20); e.close(); dt=time.time()-t0
    ok("speed limit", 1.4<dt<5, f"256 MiB @128 MiB/s: {dt:.1f} s")
    # emergency rescue
    rec=f"{WORK}/acil"; shutil.rmtree(rec,ignore_errors=True)
    e=Engine(source=d.img); e.events.clear(); e.call("scan",opts={"mode":"quick","rescue_dest":rec,"rescue_limit":1<<30}); r=e.wait_event("recover_done"); e.wait_event("scan_done"); e.close()
    got={sha(os.path.join(a,x)) for a,_,fs in os.walk(rec) for x in fs if not x.endswith(".csv")}
    n=sum(v in got for v in h.values())
    ok("emergency rescue", n>=len(h)-2, f"{n}/{len(h)}")
    os.remove(d.img)

def t_hold_trim():
    d=Disk("trim",512); d.mkfs("mkfs.btrfs","-q","-f"); d.mount("-o","discard=async")
    opts=lambda: subprocess.run(["findmnt","-rn","-S",d.loop,"-o","OPTIONS"],capture_output=True,text=True).stdout
    import threading
    before=opts()
    p=subprocess.Popen(["sudo","-S","-p","",T.ENGINE,"serve","--source",d.loop,"--hold-trim"],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
    p.stdin.write(T.PW+"\n"); p.stdin.flush(); time.sleep(1.2)
    during=opts()
    p.stdin.close(); p.wait(5); time.sleep(.8); after=opts()
    ok("TRIM protection turns on", "discard=async" in before and "discard" not in during, during.split(",")[:9].__str__()[-60:])
    ok("TRIM protection is restored", "discard=async" in after, "")
    sudo("umount",d.mnt); d.clear_automounts(); d.detach(); os.remove(d.img)

def main():
    corpus=os.path.join(WORK,"corpus"); shutil.rmtree(corpus,ignore_errors=True); h=T.make_corpus(corpus)
    only=sys.argv[1:]
    for name,fn in [("fragment",t_fragment),("bad",t_bad_sectors),("imgres",t_image_resume),("luks",lambda:t_luks_lvm(corpus,h)),("feat",lambda:t_scan_features(corpus,h)),("trim",t_hold_trim)]:
        if only and name not in only: continue
        try: fn()
        except Exception as ex:
            import traceback; traceback.print_exc(); R[name]=False; print("[ERROR]",name,ex)
    print(f"\nResult: {sum(R.values())}/{len(R)}"); return 0 if all(R.values()) else 1
sys.exit(main())

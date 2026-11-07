#!/usr/bin/env python3
"""Rent, watch and destroy the autoresearch rig on vast.ai (REST API, no CLI).

Key: VAST_API_KEY env or ~/.config/vastai/vast_api_key (chmod 600).

  vast.py offers  [--gpus 4]                list cheapest reliable RTX 3090 rigs
  vast.py rent    OFFER_ID [--disk 120]     create the instance (image = prod pod image)
  vast.py show                              state, ssh, $/h and cost so far
  vast.py watch   --cap 25                  destroy the instance once cost reaches the cap
  vast.py destroy                           destroy it now

State (instance id, start time, $/h) in ~/.local/state/ears-autoresearch/rig.json.
"""
import argparse, json, os, sys, time, urllib.parse, urllib.request

API = "https://console.vast.ai/api/v0"
IMAGE = "vllm/vllm-openai:v0.28.0"
STATE = os.path.expanduser("~/.local/state/ears-autoresearch/rig.json")


def key():
    k = os.environ.get("VAST_API_KEY")
    if not k:
        p = os.path.expanduser("~/.config/vastai/vast_api_key")
        k = open(p).read().strip() if os.path.exists(p) else None
    if not k:
        sys.exit("no API key: set VAST_API_KEY or write ~/.config/vastai/vast_api_key")
    return k


def call(method, path, body=None, auth=True, query=None):
    url = API + path + ("?" + urllib.parse.urlencode(query) if query else "")
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data, method=method, headers={"Content-Type": "application/json"})
    if auth:
        req.add_header("Authorization", "Bearer " + key())
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read() or b"{}")


def state(new=None):
    if new is not None:
        os.makedirs(os.path.dirname(STATE), exist_ok=True)
        json.dump(new, open(STATE, "w"))
        return new
    return json.load(open(STATE)) if os.path.exists(STATE) else {}


def offers(a):
    q = {"rentable": {"eq": True}, "gpu_name": {"eq": "RTX 3090"}, "num_gpus": {"eq": a.gpus},
         "reliability2": {"gte": 0.97}, "inet_down": {"gte": 300}, "disk_space": {"gte": 120},
         "order": [["dph_total", "asc"]], "type": "on-demand", "limit": 20}
    for x in call("GET", "/bundles/", auth=False, query={"q": json.dumps(q)})["offers"][:10]:
        print(f"{x['id']}  {x['num_gpus']}x3090  ${x['dph_total']:.3f}/h  {x.get('geolocation')}  "
              f"rel={x.get('reliability2', 0):.3f} cpu={x.get('cpu_cores_effective', 0):.0f} "
              f"down={x.get('inet_down', 0):.0f}Mb/s cuda={x.get('cuda_max_good')}")


def rent(a):
    if state().get("id"):
        sys.exit(f"already renting instance {state()['id']}; destroy it first")
    r = call("PUT", f"/asks/{a.offer}/", {"client_id": "me", "image": IMAGE, "disk": a.disk,
                                         "runtype": "ssh", "label": "ears-autoresearch"})
    iid = r.get("new_contract")
    if not iid:
        sys.exit(f"rent failed: {r}")
    inst = call("GET", f"/instances/{iid}/").get("instances", {})
    state({"id": iid, "start": time.time(), "dph": inst.get("dph_total")})
    print(f"rented instance {iid}; run `vast.py show` for ssh once it is running")


def show(a=None):
    s = state()
    if not s.get("id"):
        print("no rig"); return None
    inst = call("GET", f"/instances/{s['id']}/").get("instances", {})
    dph = inst.get("dph_total") or s.get("dph") or 0
    if dph and not s.get("dph"):
        s["dph"] = dph; state(s)
    hours = (time.time() - s["start"]) / 3600
    cost = hours * dph * 1.05  # 5 % margin for storage and bandwidth
    print(f"instance {s['id']} {inst.get('actual_status')} ssh -p {inst.get('ssh_port')} root@{inst.get('ssh_host')} "
          f"${dph:.3f}/h  {hours:.2f} h  ~${cost:.2f}")
    return cost


def destroy(a=None):
    s = state()
    if not s.get("id"):
        print("no rig"); return
    call("DELETE", f"/instances/{s['id']}/")
    print(f"destroyed {s['id']}")
    state({})


def watch(a):
    while state().get("id"):
        try:
            cost = show()
            if cost is not None and cost >= a.cap:
                print(f"cap ${a.cap} reached"); destroy(); return
        except Exception as e:  # keep watching through API hiccups
            print("watch:", e, file=sys.stderr)
        time.sleep(300)


ap = argparse.ArgumentParser()
sub = ap.add_subparsers(dest="cmd", required=True)
p = sub.add_parser("offers"); p.add_argument("--gpus", type=int, default=4); p.set_defaults(f=offers)
p = sub.add_parser("rent"); p.add_argument("offer"); p.add_argument("--disk", type=int, default=120); p.set_defaults(f=rent)
sub.add_parser("show").set_defaults(f=show)
p = sub.add_parser("watch"); p.add_argument("--cap", type=float, required=True); p.set_defaults(f=watch)
sub.add_parser("destroy").set_defaults(f=destroy)
a = ap.parse_args(); a.f(a)

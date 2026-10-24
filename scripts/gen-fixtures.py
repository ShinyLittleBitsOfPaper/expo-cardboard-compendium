# Regenerates crates/compiler/tests/fixtures (run from that directory).
import json
def u(n): return "00000000-0000-4000-8000-%012d" % n
def O(n): return "11111111-1111-4111-8111-%012d" % n
def img(size, side, id, stamp):
    ext = "png" if size=="png" else "jpg"
    return f"https://cards.scryfall.io/{size}/{side}/{id[0]}/{id[1]}/{id}.{ext}?{stamp}"
def imgs(id, side, stamp):
    return {k: img(k, side, id, stamp) for k in ["small","normal","large","png","art_crop","border_crop"]}
LEG = {f:"legal" for f in ["standard","pioneer","modern","legacy","vintage","commander"]}
LEG.update({"pauper":"not_legal","brawl":"banned","oldschool":"restricted"})
def base(i, oid, name, **kw):
    id = u(i)
    c = {"object":"card","id":id,"oracle_id":oid,"multiverse_ids":[1000+i],"name":name,"lang":"en",
         "released_at":"2021-07-23","uri":f"https://api.scryfall.com/cards/{id}","layout":"normal",
         "highres_image":True,"image_status":"highres_scan","cmc":1.0,"legalities":dict(LEG),"games":["paper"],
         "reserved":False,"foil":True,"nonfoil":True,"finishes":["nonfoil","foil"],"oversized":False,
         "promo":False,"reprint":False,"variation":False,"set":"m11","set_name":"Magic 2011","set_type":"core",
         "collector_number":str(100+i),"digital":False,"rarity":"common","artist":"Some Artist",
         "border_color":"black","frame":"2015","full_art":False,"textless":False,"booster":True,
         "prices":{"usd":"0.10","usd_foil":None},"edhrec_rank":500+i,
         "purchase_uris":{"tcgplayer":f"https://www.tcgplayer.com/product/{i}"}}
    c.update(kw); return c
def normal(i, oid, name, text, colors, **kw):
    c = base(i,oid,name,mana_cost="{R}",type_line="Instant",oracle_text=text,colors=colors,color_identity=colors,
             keywords=[],image_uris=imgs(u(i),"front",1700000000+i))
    c.update(kw); return c
A=[]
A.append(normal(1,O(1),"Lightning Bolt","Lightning Bolt deals 3 damage to any target.",["R"]))
A.append(normal(2,O(1),"Lightning Bolt","Lightning Bolt deals 3 damage to any target.",["R"],reprint=True,promo=True,promo_types=["promopack"],set="p23",set_name="Promo Pack",flavor_text="Zap!"))
id=u(3)
A.append(base(3,O(2),"Delver of Secrets // Insectile Aberration",layout="transform",type_line="Creature — Human Wizard // Creature — Human Insect",color_identity=["U"],keywords=["Transform"],cmc=1.0,
  card_faces=[{"object":"card_face","name":"Delver of Secrets","mana_cost":"{U}","type_line":"Creature — Human Wizard","oracle_text":"At the beginning of your upkeep, look at the top card.","power":"1","toughness":"1","colors":["U"],"artist":"Nils Hamm","flavor_text":"Front flavor","image_uris":imgs(id,"front",1700000003)},
              {"object":"card_face","name":"Insectile Aberration","mana_cost":"","type_line":"Creature — Human Insect","oracle_text":"Flying","power":"3","toughness":"2","colors":["U"],"artist":"Nils Hamm","image_uris":imgs(id,"back",1700000003)}]))
id=u(4)
A.append(base(4,O(3),"Fire // Ice",layout="split",mana_cost="{1}{R} // {1}{U}",type_line="Instant // Instant",colors=["R","U"],color_identity=["R","U"],cmc=4.0,keywords=[],image_uris=imgs(id,"front",1700000004),
  card_faces=[{"object":"card_face","name":"Fire","mana_cost":"{1}{R}","type_line":"Instant","oracle_text":"Fire deals 2 damage divided as you choose."},
              {"object":"card_face","name":"Ice","mana_cost":"{1}{U}","type_line":"Instant","oracle_text":"Tap target permanent."}]))
id=u(5)
A.append(base(5,O(4),"Bonecrusher Giant // Stomp",layout="adventure",mana_cost="{2}{R}",type_line="Creature — Giant",colors=["R"],color_identity=["R"],cmc=3.0,power="4",toughness="3",keywords=["Adventure"],image_uris=imgs(id,"front",1700000005),
  card_faces=[{"object":"card_face","name":"Bonecrusher Giant","mana_cost":"{2}{R}","type_line":"Creature — Giant","oracle_text":"Whenever this becomes the target...","power":"4","toughness":"3"},
              {"object":"card_face","name":"Stomp","mana_cost":"{1}{R}","type_line":"Instant — Adventure","oracle_text":"Damage can't be prevented this turn."}]))
id=u(6)
r=base(6,O(5),"Reversible // Card",layout="reversible_card",type_line="Creature // Creature",cmc=2.0,keywords=[],
  card_faces=[{"object":"card_face","oracle_id":O(5),"name":"Reversible","mana_cost":"{1}{G}","type_line":"Creature","oracle_text":"x","power":"2","toughness":"2","colors":["G"],"image_uris":imgs(id,"front",1700000006)},
              {"object":"card_face","oracle_id":O(5),"name":"Card","mana_cost":"{1}{G}","type_line":"Creature","oracle_text":"y","power":"2","toughness":"2","colors":["G"],"image_uris":imgs(id,"back",1700000006)}])
r.pop("oracle_id"); A.append(r)
A.append(normal(7,O(6),"Digital Only","x",["R"],digital=True))
A.append(base(8,O(7),"Art Card",layout="art_series",image_uris=imgs(u(8),"front",1)))
x=normal(9,O(8),"No Oracle","x",["R"]); x.pop("oracle_id"); A.append(x)
x=normal(10,O(9),"Bad Id","x",["R"]); x["id"]="not-a-uuid"; A.append(x)
A.append(normal(11,O(10),"Little Girl","Half mana.",["W"],mana_cost="{W/2}",cmc=0.5,type_line="Creature — Human",rarity="rare"))
A.append(normal(12,O(11),"Lim-Dûl's Vault","Look at the top five cards.",["U","B"],lang="ja",image_uris={"normal":"https://example.com/x.jpg"},finishes=["nonfoil","foil","etched"],printed_name="リム＝ドゥールの宝物庫"))
A.append(normal(13,O(12),"Colorless Rock","Tap: add one mana.",[],type_line="Artifact",promo_types=["serialized","boosterfun"],finishes=["foil"]))
x=normal(14,O(13),"Weird One","Odd.",["G","W"],finishes=["glossy","nonfoil"],future_field={"a":[1,2,{"b":None}]},watermark=None)
x["legalities"]=dict(LEG, newformat="legal")
A.append(x)
import re
def slug(n):
    n=n.lower().replace("'","").replace("\u2019","")
    return re.sub(r"[^a-z0-9]+","-",n).strip("-")
def fix(cards):
    """Add the URL keys Scryfall derives from other fields, like real data has."""
    for c in cards:
        oid = c.get("oracle_id") or (c.get("card_faces") or [{}])[0].get("oracle_id", "00000000-0000-4000-8000-000000000000")
        c["rulings_uri"]=f"https://api.scryfall.com/cards/{c['id']}/rulings"
        c["prints_search_uri"]=f"https://api.scryfall.com/cards/search?order=released&q=oracleid%3A{oid}&unique=prints"
        c["scryfall_uri"]=f"https://scryfall.com/card/{c['set']}/{c['collector_number']}/{slug(c['name'])}?utm_source=api"
        if c["id"]==u(12):  # non-ASCII name: Scryfall's slug differs from ours, so this one must be stored
            c["scryfall_uri"]="https://scryfall.com/card/m11/112/lim-dul-s-vault?utm_source=api"
fix(A)
with open("sample_a.jsonl","w") as f:
    for c in A: f.write(json.dumps(c,ensure_ascii=False)+"\n")
B=[json.loads(json.dumps(c)) for c in A if c["id"]!=u(2)]
B.append(normal(15,O(1),"Lightning Bolt","Lightning Bolt deals 3 damage to any target.",["R"],set="2xm",set_name="Double Masters",released_at="2025-01-01",reprint=True))
B.append(normal(16,O(14),"Brand New Card","New.",["B"]))
for c in B:
    if c["id"]==u(11): c["oracle_text"]="Half mana, errata."
    if c["id"]==u(1): c["legalities"]=dict(LEG, pauper="legal")
fix(B)
with open("sample_b.jsonl","w") as f:
    for c in B: f.write(json.dumps(c,ensure_ascii=False)+"\n")
json.dump(A, open("sample_a_array.json","w"), ensure_ascii=False)
print(len(A), len(B))

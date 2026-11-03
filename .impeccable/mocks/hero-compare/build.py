"""Builds the hero comparison as one self-contained page.

Needs the synthetic preview running (see README) and Google Chrome:

    python3 .impeccable/mocks/hero-compare/build.py [http://127.0.0.1:3100]

Writes target/hero-compare.html (published as the Claude artifact) and a copy
at target/site/hero-compare.html, served by the preview. The page takes the
built stylesheet with Inter and the seal masks inlined; TX-02 is licensed,
so it stays out and figures fall back to a system monospace. The directory
is the preview's, as Chrome renders it after the list loads, without the
app's scripts or remote images.
"""
import base64
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.parse
from pathlib import Path

here = Path(__file__).resolve().parent
repo = here.parents[2]
url = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:3100/"
chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

css = (repo / "target/site/pkg/cdk-cashu-auditor.css").read_text()
css = re.sub(r"@font-face\s*{[^}]*tx-02[^}]*}", "", css, flags=re.I)
css = re.sub(r"@font-face\s*{[^}]*InterVariable-Italic[^}]*}", "", css)
inter = base64.b64encode((repo / "public/fonts/InterVariable.woff2").read_bytes()).decode()
css = css.replace('url("/fonts/InterVariable.woff2")', f'url("data:font/woff2;base64,{inter}")')
for name in ("imf-seal-mask.svg", "imf-shield-mask.svg"):
    svg = (repo / "public/brand" / name).read_text().strip()
    css = css.replace(f'url("/brand/{name}?v=2")', f'url("data:image/svg+xml,{urllib.parse.quote(svg, safe="")}")')
leftover = [u for u in re.findall(r"url\(([^)]*)\)", css) if "data:" not in u]
assert not leftover, f"unresolved urls: {leftover}"

with tempfile.TemporaryDirectory() as profile:
    dom = subprocess.run(
        [chrome, "--headless=new", "--disable-gpu", f"--user-data-dir={profile}",
         "--virtual-time-budget=8000", "--dump-dom", url],
        capture_output=True, text=True, check=True,
    ).stdout
body = re.search(r"<body[^>]*>(.*)</body>", dom, flags=re.S).group(1)
body = re.sub(r"<script\b[^>]*>.*?</script>", "", body, flags=re.S)
body = re.sub(r"<img\b[^>]*>", "", body)
body = re.sub(r"<!--.*?-->", "", body, flags=re.S)
assert "masthead" in body and 'class="mint-row' in body, "the directory didn't render"

page = (here / "page.html").read_text().replace("/*APP_CSS*/", css).replace("<!--PAGE-->", body)
out = repo / "target/hero-compare.html"
out.write_text(page)
shutil.copy(out, repo / "target/site/hero-compare.html")
print(f"{out.relative_to(repo)}: {len(page):,} bytes")

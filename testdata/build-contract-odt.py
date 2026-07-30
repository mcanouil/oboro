"""Rebuilds testdata/contract.odt.

Written as a single line per part, the way a real producer writes it, so the
reader's handling of a producer's indentation is not what the fixture tests.
"""

import zipfile

NS = (
    'xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"'
    ' xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"'
    ' xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"'
    ' xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"'
    ' xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"'
    ' xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"'
    ' xmlns:dc="http://purl.org/dc/elements/1.1/"'
    ' xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"'
)

BODY = (
    "<text:h>Acme Consulting SARL</text:h>"
    "<text:p><text:span>Jean Dupont</text:span>, gerant</text:p>"
    "<text:p>jean.dupont@acme-consulting.example</text:p>"
    "<text:p>06 12 34 56 78</text:p>"
    "<text:p>IBAN<text:s text:c=\"3\"/>FR14 2004 1010 0505 0001 3M02 606</text:p>"
    "<text:p>Carte 4242 4242 4242 4242</text:p>"
    "<text:p>Reference CT-874512</text:p>"
    # An annotation anchored mid-paragraph: its metadata is bare character
    # data, including meta:date-string, which no name list had heard of.
    "<text:p>12 bis rue de la Paix"
    "<office:annotation>"
    "<dc:creator>Jean Dupont</dc:creator>"
    "<dc:date>2026-07-30T08:15:29</dc:date>"
    "<meta:date-string>30/07/2026 08:15</meta:date-string>"
    "<text:p>marie.martin@globex.example</text:p>"
    "</office:annotation>, 75002 Paris</text:p>"
    # LibreOffice's alt text on an image: bare character data inside the
    # paragraph the image is anchored in, with an image's bytes beside it.
    "<text:p>Signe par"
    "<draw:frame><svg:title>Globex Industries</svg:title>"
    "<svg:desc>photo prise a Lille</svg:desc>"
    "<draw:image><office:binary-data>iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB</office:binary-data>"
    "</draw:image></draw:frame> le 30/07/2026</text:p>"
    # A footnote: its marker is character data directly inside text:note.
    "<text:p>Tarifs<text:note text:note-class=\"footnote\">"
    "<text:note-citation>1</text:note-citation>"
    "<text:note-body><text:p>8 avenue des Champs-&#201;lys&#233;es</text:p></text:note-body>"
    "</text:note> revises</text:p>"
    # Entity references inside bounded text: these used to shatter across
    # lines, which is how a denylisted name reached the output whole.
    "<office:annotation>"
    "<dc:creator>Dupont &amp; Fils</dc:creator>"
    "<meta:date-string>Jos&#233;phine</meta:date-string>"
    "<text:p>Soci&#233;t&#233; enregistree</text:p>"
    "</office:annotation>"
    # An inline field, to prove text:title is not confused with svg:title.
    "<text:p>Objet : <text:title>Contrat</text:title>, tel +33 1 42 68 53 00</text:p>"
    "<table:table><table:table-row><table:table-cell>"
    "<text:p>SIRET 12345678200002</text:p>"
    "</table:table-cell></table:table-row></table:table>"
)

CONTENT = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    f"<office:document-content {NS} office:version=\"1.3\">"
    f"<office:body><office:text>{BODY}</office:text></office:body>"
    "</office:document-content>"
)

STYLES = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    f"<office:document-styles {NS} office:version=\"1.3\">"
    "<office:master-styles><style:master-page style:name=\"Standard\">"
    "<style:header><text:p>Acme Consulting SARL</text:p></style:header>"
    "<style:footer><text:p>192.168.14.201</text:p></style:footer>"
    "</style:master-page></office:master-styles>"
    "</office:document-styles>"
)

MANIFEST = (
    '<?xml version="1.0" encoding="UTF-8"?>'
    '<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"'
    ' manifest:version="1.3">'
    '<manifest:file-entry manifest:full-path="/"'
    ' manifest:media-type="application/vnd.oasis.opendocument.text"/>'
    '<manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/>'
    '<manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/>'
    "</manifest:manifest>"
)

with zipfile.ZipFile("testdata/contract.odt", "w", zipfile.ZIP_DEFLATED) as archive:
    # The mimetype entry is first and stored, as OpenDocument requires.
    archive.writestr(
        zipfile.ZipInfo("mimetype"),
        "application/vnd.oasis.opendocument.text",
        compress_type=zipfile.ZIP_STORED,
    )
    archive.writestr("META-INF/manifest.xml", MANIFEST)
    archive.writestr("content.xml", CONTENT)
    archive.writestr("styles.xml", STYLES)

print("testdata/contract.odt written")

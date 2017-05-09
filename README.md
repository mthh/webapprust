### Convert to GML/GeoJSON as a service

Example server application using [Iron](https://github.com/iron/iron) and a few middlewares
(staticfile, basic logging, *handlebars* templating system, custom 404 page, ...).

Post a geographic layer (KML, GeoJSON, ShapeFile, GML) and get directly the transformed layer
in Geographic Markup Language (GML) or GeoJSON.

Rely on *ogr2ogr*.  
No real usecase so far.  
Shamelessly inspired by [html2pdf](https://github.com/rap2hpoutre/htmltopdf).

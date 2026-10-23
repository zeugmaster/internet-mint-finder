FROM alpine:3.20
RUN apk add --no-cache tor && mkdir -p /var/lib/tor && chown -R tor:tor /var/lib/tor
COPY torrc /etc/tor/torrc
USER tor
VOLUME ["/var/lib/tor"]
CMD ["tor", "-f", "/etc/tor/torrc"]

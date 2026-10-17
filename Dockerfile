FROM oven/bun:1.4.3-alpine AS build
ENV VUE_APP_NETEASE_API_URL=/api
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile --ignore-scripts
COPY . .
RUN bun run build:renderer

FROM nginx:1.20.2-alpine AS app

RUN apk add --no-cache libuv nodejs npm \
  && npm i -g @neteaseapireborn/api@4.29.7

COPY --from=build /app/docker/nginx.conf.example /etc/nginx/conf.d/default.conf
COPY --from=build /app/dist /usr/share/nginx/html

CMD ["sh", "-c", "nginx && exec api"]

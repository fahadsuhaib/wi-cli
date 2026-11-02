FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      openssh-server tmux vim htop procps tar ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd \
    && useradd -m -s /bin/bash dev \
    && mkdir -p /home/dev/.ssh
COPY id_test.pub /home/dev/.ssh/authorized_keys
RUN chown -R dev:dev /home/dev/.ssh && chmod 700 /home/dev/.ssh && chmod 600 /home/dev/.ssh/authorized_keys \
    && printf 'PasswordAuthentication no\nPermitRootLogin no\n' > /etc/ssh/sshd_config.d/devbox.conf
EXPOSE 22
CMD ["/usr/sbin/sshd","-D","-e"]

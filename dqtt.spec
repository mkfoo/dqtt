Name:           dqtt
Version:        0.1.0        
Release:        0
Summary:        DQTT messaging protocol (server)
License:        MIT 

%description
DQTT messaging protocol (server)

%build
cross build --target aarch64-unknown-linux-gnu --release

%install
install -D -m 0755 target/aarch64-unknown-linux-gnu/release/%{name} %{buildroot}%{_bindir}/%{name}
install -D -m 0644 %{name}.service %{buildroot}%{_unitdir}/%{name}.service 

%pre
getent group %{name} || groupadd -r %{name}
getent passwd %{name} || useradd -r -g %{name} -s /bin/false %{name}
%service_add_pre %{name}.service

%preun
%service_del_preun %{name}.service

%post
%service_add_post %{name}.service

%postun
%service_del_postun %{name}.service

%files
%{_bindir}/%{name}
%{_unitdir}/%{name}.service
